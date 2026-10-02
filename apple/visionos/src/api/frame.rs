use super::{
    metal_texture::{import_color_texture, import_depth_texture},
    *,
};

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
