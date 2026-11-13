use super::{
    metal_texture::{import_color_texture, import_depth_texture},
    *,
};

mod anchor;
use anchor::{solve_anchor_to_screen, AnchorCamera};

/// Draws every eye of a frame and returns the Metal texture (`id<MTLTexture>`) the map draws
/// into when an eye has no colour texture of its own, owned by the map and valid until the
/// next call; null when the frame could not be drawn. Depth textures are written before this
/// returns.
///
/// `request_overscan` widens each eye's frustum for tile requests, one for none.
/// `timestamp_seconds` drives animations and must not decrease.
///
/// # Safety
///
/// `map` must come from [`maplibre_visionos_create`]; `placement` must point at a struct
/// whose matrix pointer addresses 16 floats; `eyes` must point at `eye_count` structs whose
/// matrix pointers address 16 floats and tangent pointers 4, all alive for the call; the
/// textures, if any, must be of the map's size and stay alive for the call.
#[no_mangle]
pub unsafe extern "C" fn maplibre_visionos_render_frame(
    map: *mut MaplibreVisionOSMap,
    placement: *const MaplibreVisionOSPlacement,
    prefetch: *const MaplibreVisionOSPlacement,
    eyes: *const MaplibreVisionOSEye,
    eye_count: u32,
    request_overscan: f32,
    timestamp_seconds: f64,
) -> *const c_void {
    // SAFETY: the caller upholds the pointer contracts documented above.
    let (Some(handle), Some(placement)) = (unsafe { map.as_mut() }, unsafe { placement.as_ref() })
    else {
        return ptr::null();
    };
    if placement.world_from_scene.is_null() || eyes.is_null() || eye_count == 0 {
        return ptr::null();
    }
    // SAFETY: as above, the placement matrix holds 16 floats and `eyes` holds `eye_count` eyes.
    let (world_from_scene, eyes) = unsafe {
        (
            std::slice::from_raw_parts(placement.world_from_scene, 16),
            std::slice::from_raw_parts(eyes, eye_count as usize),
        )
    };
    let mut frame_eyes = Vec::with_capacity(eyes.len());
    for eye in eyes {
        if eye.world_from_eye.is_null() || eye.tangents.is_null() {
            return ptr::null();
        }
        // SAFETY: as above, the matrix holds 16 floats and the tangents 4.
        let (world_from_eye, tangents) = unsafe {
            (
                std::slice::from_raw_parts(eye.world_from_eye, 16),
                std::slice::from_raw_parts(eye.tangents, 4),
            )
        };
        let near = f64::from(eye.near);
        // A compositor that reprojects reports no far plane. The flat map's plane reaches the
        // far plane, and the sky starts at the horizon, so the far plane sits close enough to
        // the horizon that the gap between the two stays under a pixel.
        let far = if eye.far.is_finite() && eye.far > eye.near {
            f64::from(eye.far)
        } else {
            near * FAR_OVER_NEAR
        };
        // SAFETY: the caller keeps the textures alive for the call.
        let (color, depth) = unsafe {
            (
                import_color_texture(handle.map.device(), eye.color_texture),
                import_depth_texture(handle.map.device(), eye.depth_texture),
            )
        };
        frame_eyes.push(XrEye {
            world_from_eye: column_major(world_from_eye),
            frustum: EyeFrustum {
                left: f64::from(tangents[0]),
                right: f64::from(tangents[1]),
                top: f64::from(tangents[2]),
                bottom: f64::from(tangents[3]),
                near,
                far,
            },
            target: EyeTarget { color, depth },
        });
    }
    // SAFETY: as above; a null prefetch means no flight is in progress.
    let prefetch = unsafe { prefetch.as_ref() }.and_then(|ahead| {
        if ahead.world_from_scene.is_null() {
            return None;
        }
        // SAFETY: as above, the matrix holds 16 floats.
        let matrix = unsafe { std::slice::from_raw_parts(ahead.world_from_scene, 16) };
        Some(ScenePlacement {
            anchor: ExternalAnchor {
                position: LatLon::new(ahead.anchor_latitude, ahead.anchor_longitude),
                altitude_meters: ahead.anchor_altitude_meters,
            },
            world_from_scene: column_major(matrix),
        })
    });
    let frame = XrFrame {
        opaque_environment: handle.opaque_environment,
        timestamp: Duration::from_secs_f64(timestamp_seconds.max(0.0)),
        placement: ScenePlacement {
            anchor: ExternalAnchor {
                position: LatLon::new(placement.anchor_latitude, placement.anchor_longitude),
                altitude_meters: placement.anchor_altitude_meters,
            },
            world_from_scene: column_major(world_from_scene),
        },
        eyes: frame_eyes,
        request_overscan: f64::from(request_overscan),
        prefetch,
    };
    {
        let _runtime = handle.runtime.enter();
        if let Err(error) = handle.map.run_xr_frame(frame) {
            tracing::error!(%error, "frame failed");
            return ptr::null();
        }
    }
    handle.frames = handle.frames.wrapping_add(1);
    // Where the map looks, now and then, so a host can check its placement.
    if handle.frames == 1 || handle.frames % 300 == 0 {
        let view_state = handle.map.view_state();
        let center = view_state.external_view().anchor.position;
        tracing::info!(
            frame = handle.frames,
            zoom = view_state.zoom().value(),
            center_latitude = center.latitude,
            center_longitude = center.longitude,
            pitch = view_state.camera().get_pitch().0.to_degrees(),
            "map view"
        );
    }
    // The host copies and presents on the map's own queue, in order after these draws, so no
    // wait is needed here; polling only runs the callbacks of finished work.
    if let Err(error) = handle.map.device().poll(wgpu::PollType::Poll) {
        tracing::error!(%error, "GPU polling failed");
        return ptr::null();
    }
    let Some(texture) = handle.map.head_texture() else {
        return ptr::null();
    };
    // SAFETY: the map owns the texture, keeping the borrowed Metal object alive.
    unsafe { texture.as_hal::<wgpu::hal::api::Metal>() }.map_or(ptr::null(), |texture| {
        ptr::from_ref(texture.raw_handle()).cast()
    })
}

/// Draws one frame from the map's orbit / free-globe camera (`MapView`) and returns the
/// map's offscreen `id<MTLTexture>`, owned by the map and valid until the next draw call.
///
/// # Safety
///
/// `map` must come from [`maplibre_visionos_create`] and not have been destroyed.
#[no_mangle]
pub unsafe extern "C" fn maplibre_visionos_render_orbit(
    map: *mut MaplibreVisionOSMap,
    latitude: f64,
    longitude: f64,
    zoom: f64,
    pitch_deg: f64,
    bearing_deg: f64,
    roll_deg: f64,
    timestamp_seconds: f64,
) -> *const c_void {
    // SAFETY: the caller upholds the map handle contract documented above.
    let Some(handle) = (unsafe { map.as_mut() }) else {
        return ptr::null();
    };
    handle.map.clear_external_view();
    handle.map.set_opaque_environment(handle.opaque_environment);
    handle
        .map
        .set_navigation_mode(maplibre::render::view_state::NavigationMode::FreeGlobe)
        .ok();
    if let Err(error) = handle
        .map
        .jump_to(maplibre::render::view_state::CameraOptions {
            center: Some(LatLon::new(latitude, longitude)),
            zoom: Some(zoom),
            pitch: Some(Deg(pitch_deg)),
            bearing: Some(Deg(bearing_deg)),
            roll: Some(Deg(roll_deg)),
            ..maplibre::render::view_state::CameraOptions::default()
        })
    {
        tracing::error!(%error, "orbit jump_to failed");
        return ptr::null();
    }
    handle.map.frame_input_mut().timestamp = Duration::from_secs_f64(timestamp_seconds.max(0.0));
    {
        let _runtime = handle.runtime.enter();
        if let Err(error) = handle.map.run_frame() {
            tracing::error!(%error, "orbit frame failed");
            return ptr::null();
        }
    }
    handle.frames = handle.frames.wrapping_add(1);
    if let Err(error) = handle.map.device().poll(wgpu::PollType::Poll) {
        tracing::error!(%error, "GPU polling failed");
        return ptr::null();
    }
    let Some(texture) = handle.map.head_texture() else {
        return ptr::null();
    };
    // SAFETY: the map owns the texture, keeping the borrowed Metal object alive.
    unsafe { texture.as_hal::<wgpu::hal::api::Metal>() }.map_or(ptr::null(), |texture| {
        ptr::from_ref(texture.raw_handle()).cast()
    })
}

/// Returns the map's effective style zoom (matching eye-height zoom for 6-DoF external frames
/// and map zoom for orbit frames), or `0.0` when `map` is null.
///
/// # Safety
///
/// `map` must come from [`maplibre_visionos_create`] and not have been destroyed, or be null.
#[no_mangle]
pub unsafe extern "C" fn maplibre_visionos_effective_zoom(map: *const MaplibreVisionOSMap) -> f64 {
    // SAFETY: the caller upholds the map handle contract documented above.
    unsafe { map.as_ref() }.map_or(0.0, |handle| handle.map.view_state().style_zoom().value())
}

/// Raycasts a logical viewport point `(screen_x, screen_y)` onto the globe / loaded DEM surface,
/// writing `(latitude, longitude, elevation_meters)` when the ray intersects the planet.
///
/// # Safety
///
/// `map` must be valid or null; output pointers must be valid writable `f64` pointers.
#[no_mangle]
pub unsafe extern "C" fn maplibre_visionos_screen_to_ground(
    map: *const MaplibreVisionOSMap,
    screen_x: f64,
    screen_y: f64,
    out_latitude: *mut f64,
    out_longitude: *mut f64,
    out_elevation_meters: *mut f64,
) -> bool {
    let Some(handle) = (unsafe { map.as_ref() }) else {
        return false;
    };
    if out_latitude.is_null() || out_longitude.is_null() || out_elevation_meters.is_null() {
        return false;
    }
    let view_state = handle.map.view_state();
    let Ok(camera) = maplibre::render::projection::globe_camera_for_view(view_state) else {
        return false;
    };
    let px = cgmath::Point2::new(screen_x, screen_y);
    if !camera.is_point_on_map_surface(px) {
        return false;
    }
    let base_elev = view_state.center_elevation();
    let Some(loc0) = camera
        .screen_point_to_location_at(px, base_elev)
        .or_else(|| camera.screen_point_to_location(px))
    else {
        return false;
    };
    let (loc, elev) = if let Some(dem) = handle.map.context().loaded_ground_cached(loc0) {
        let refined = camera.screen_point_to_location_at(px, dem).unwrap_or(loc0);
        (refined, dem)
    } else {
        (loc0, base_elev)
    };
    // SAFETY: null checks performed above; caller owns the output scalars.
    unsafe {
        *out_latitude = loc.latitude;
        *out_longitude = loc.longitude;
        *out_elevation_meters = elev;
    }
    true
}

/// Moves the free-globe orbit camera so the 3D ground anchor `(anchor_lat, anchor_lon, anchor_elev_m)`
/// projects directly under logical viewport point `(screen_x, screen_y)` while holding `(zoom, pitch_deg,
/// bearing_deg)`. Returns `false` without mutating the camera when `(screen_x, screen_y)` lies outside
/// the visible planetary limb or the anchor rotates past the horizon, allowing the caller to hand off
/// seamlessly to [`maplibre_visionos_drag_orbit`] on the same frame.
///
/// # Safety
///
/// `map` must be valid or null; output pointers must be valid writable `f64` pointers.
#[no_mangle]
pub unsafe extern "C" fn maplibre_visionos_move_anchor_to_screen(
    map: *mut MaplibreVisionOSMap,
    anchor_lat: f64,
    anchor_lon: f64,
    anchor_elev_m: f64,
    screen_x: f64,
    screen_y: f64,
    zoom: f64,
    pitch_deg: f64,
    bearing_deg: f64,
    north_locked: bool,
    out_center_lat: *mut f64,
    out_center_lon: *mut f64,
    out_bearing_deg: *mut f64,
) -> bool {
    let Some(handle) = (unsafe { map.as_mut() }) else {
        return false;
    };
    if out_center_lat.is_null() || out_center_lon.is_null() || out_bearing_deg.is_null() {
        return false;
    }
    let Some((lat, lon, bearing)) = solve_anchor_to_screen(
        &mut handle.map,
        LatLon::new(anchor_lat, anchor_lon),
        anchor_elev_m,
        cgmath::Point2::new(screen_x, screen_y),
        AnchorCamera {
            zoom,
            pitch_deg,
            bearing_deg,
            north_locked,
        },
    ) else {
        return false;
    };
    // SAFETY: null checks performed above; caller owns the output scalars.
    unsafe {
        *out_center_lat = lat;
        *out_center_lon = lon;
        *out_bearing_deg = bearing;
    }
    true
}

/// Incremental globe pan from `(from_x, from_y)` to `(to_x, to_y)` supporting empty-space drags
/// outside the planetary limb, cross-limb hyperbolic trackball drags, North-Up pole-dialing when
/// `north_locked` is `true`, and unrestricted pole-crossing `FreeGlobe` rotation when `false`.
///
/// # Safety
///
/// `map` must be valid or null; output pointers must be valid writable `f64` pointers.
#[no_mangle]
pub unsafe extern "C" fn maplibre_visionos_drag_orbit(
    map: *mut MaplibreVisionOSMap,
    from_x: f64,
    from_y: f64,
    to_x: f64,
    to_y: f64,
    zoom: f64,
    pitch_deg: f64,
    bearing_deg: f64,
    north_locked: bool,
    out_center_lat: *mut f64,
    out_center_lon: *mut f64,
    out_bearing_deg: *mut f64,
) -> bool {
    let Some(handle) = (unsafe { map.as_mut() }) else {
        return false;
    };
    if out_center_lat.is_null() || out_center_lon.is_null() || out_bearing_deg.is_null() {
        return false;
    }
    handle.map.clear_external_view();
    handle
        .map
        .set_navigation_mode(maplibre::render::view_state::NavigationMode::FreeGlobe)
        .ok();
    handle
        .map
        .jump_to(maplibre::render::view_state::CameraOptions {
            zoom: Some(zoom),
            pitch: Some(Deg(pitch_deg)),
            bearing: Some(Deg(if north_locked { 0.0 } else { bearing_deg })),
            roll: Some(Deg(0.0)),
            ..maplibre::render::view_state::CameraOptions::default()
        })
        .ok();
    let from_px = cgmath::Point2::new(from_x, from_y);
    let to_px = cgmath::Point2::new(to_x, to_y);
    let moved = if north_locked {
        let Ok(camera) =
            maplibre::render::projection::globe_camera_for_view(handle.map.view_state())
        else {
            return false;
        };
        let center_px =
            camera.location_to_screen(camera.center(), handle.map.view_state().center_elevation());
        if let Some(update) = maplibre::projection::globe::interaction::pan_camera_by_pixels(
            &camera,
            to_px,
            center_px,
            to_px - from_px,
        ) {
            handle
                .map
                .jump_to(maplibre::render::view_state::CameraOptions {
                    center: Some(update.center),
                    zoom: Some(zoom),
                    pitch: Some(Deg(pitch_deg)),
                    bearing: Some(Deg(0.0)),
                    roll: Some(Deg(0.0)),
                    ..maplibre::render::view_state::CameraOptions::default()
                })
                .is_ok()
        } else {
            false
        }
    } else {
        handle.map.drag_globe(from_px, to_px)
    };
    let Some(pose) = handle.map.view_state().pose_view() else {
        return false;
    };
    // SAFETY: null checks performed above; caller owns the output scalars.
    unsafe {
        *out_center_lat = pose.center.latitude;
        *out_center_lon = pose.center.longitude;
        *out_bearing_deg = if north_locked {
            0.0
        } else {
            pose.bearing_degrees
        };
    }
    moved
}
