use super::*;

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
    handle.map.device().poll(wgpu::Maintain::Poll);
    let Some(texture) = handle.map.head_texture() else {
        return ptr::null();
    };
    // SAFETY: the renderer runs on Metal on this platform; the callback only reads the handle,
    // which stays alive with the texture the map owns. `raw_handle` comes from the vendored
    // wgpu-hal, see vendor-wgpu.sh.
    unsafe {
        texture.as_hal::<wgpu_hal::api::Metal, _, _>(|texture| {
            texture.map_or(ptr::null(), |texture| texture.raw_handle().as_ptr().cast())
        })
    }
}

/// Wraps a compositor colour texture as a linear render target for the map; `None` for a
/// null handle, a format other than `bgra8Unorm` or its sRGB twin, or a texture that cannot
/// be viewed in another format, which the host should then draw through a copy.
///
/// # Safety
///
/// `pointer` must be null or an `id<MTLTexture>` of a single-level 2D texture that stays
/// alive for the frame.
unsafe fn import_color_texture(
    device: &wgpu::Device,
    pointer: *const c_void,
) -> Option<wgpu::TextureView> {
    if pointer.is_null() {
        return None;
    }
    // SAFETY: the caller passes a live Metal texture; `to_owned` retains it.
    let raw = unsafe { metal::TextureRef::from_ptr(pointer.cast_mut().cast()) }.to_owned();
    let format = match raw.pixel_format() {
        metal::MTLPixelFormat::BGRA8Unorm => wgpu::TextureFormat::Bgra8Unorm,
        metal::MTLPixelFormat::BGRA8Unorm_sRGB => wgpu::TextureFormat::Bgra8UnormSrgb,
        other => {
            tracing::warn!(?other, "colour texture format is not drawable");
            return None;
        }
    };
    if format != SURFACE_FORMAT
        && !raw
            .usage()
            .contains(metal::MTLTextureUsage::PixelFormatView)
    {
        return None;
    }
    let (width, height) = (raw.width() as u32, raw.height() as u32);
    // SAFETY: the texture is one 2D level with one layer, which is what is described here.
    let texture = unsafe {
        let hal = wgpu_hal::metal::Device::texture_from_raw(
            raw,
            format,
            metal::MTLTextureType::D2,
            1,
            1,
            wgpu_hal::CopyExtent {
                width,
                height,
                depth: 1,
            },
        );
        device.create_texture_from_hal::<wgpu_hal::api::Metal>(
            hal,
            &wgpu::TextureDescriptor {
                label: Some("compositor colour"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[SURFACE_FORMAT],
            },
        )
    };
    Some(texture.create_view(&wgpu::TextureViewDescriptor {
        format: Some(SURFACE_FORMAT),
        ..Default::default()
    }))
}

/// Wraps the compositor's depth texture for the renderer; `None` for a null handle.
///
/// # Safety
///
/// `pointer` must be null or an `id<MTLTexture>` of a single-level `Depth32Float` texture
/// that stays alive for the frame.
unsafe fn import_depth_texture(
    device: &wgpu::Device,
    pointer: *const c_void,
) -> Option<wgpu::TextureView> {
    if pointer.is_null() {
        return None;
    }
    // SAFETY: the caller passes a live Metal texture; `to_owned` retains it, so the wrapper
    // releases its own reference when the frame is done and the host keeps its own.
    let raw = unsafe { metal::TextureRef::from_ptr(pointer.cast_mut().cast()) }.to_owned();
    let (width, height) = (raw.width() as u32, raw.height() as u32);
    // SAFETY: the texture is one 2D level with one layer, which is what is described here.
    // `texture_from_raw` comes from the vendored wgpu-hal, see vendor-wgpu.sh.
    let texture = unsafe {
        let hal = wgpu_hal::metal::Device::texture_from_raw(
            raw,
            wgpu::TextureFormat::Depth32Float,
            metal::MTLTextureType::D2,
            1,
            1,
            wgpu_hal::CopyExtent {
                width,
                height,
                depth: 1,
            },
        );
        device.create_texture_from_hal::<wgpu_hal::api::Metal>(
            hal,
            &wgpu::TextureDescriptor {
                label: Some("compositor depth"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            },
        )
    };
    Some(texture.create_view(&wgpu::TextureViewDescriptor::default()))
}
