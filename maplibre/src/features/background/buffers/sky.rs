//! Sky uniform construction for [`BackgroundBuffers`](super::BackgroundBuffers).

use crate::render::{shaders::SkyLayerMetadata, view_state::ViewState};

/// The sky draw's values for the frame, scaled to the physical frame height.
pub fn sky_metadata(
    sky: &crate::style::sky::SkySpecification,
    view_state: &ViewState,
    projection_transition: f32,
    frame_height: f32,
) -> SkyLayerMetadata {
    sky_metadata_with_camera(sky, view_state, projection_transition, frame_height, None)
}

/// Same as [`sky_metadata`], reusing an already-resolved [`GlobeCameraState`] when available.
pub fn sky_metadata_with_camera(
    sky: &crate::style::sky::SkySpecification,
    view_state: &ViewState,
    projection_transition: f32,
    frame_height: f32,
    cached_globe_camera: Option<&crate::projection::globe::camera::GlobeCameraState>,
) -> SkyLayerMetadata {
    let zoom = view_state.zoom().value();
    let colors = sky.colors_at(zoom);
    let logical_height = view_state.height();
    let scale = if logical_height > 0.0 && frame_height > 0.0 {
        f64::from(frame_height) / logical_height
    } else {
        1.0
    };
    let height = logical_height * scale;
    let mut horizon = view_state.horizon_line();
    horizon.point.x *= scale;
    horizon.point.y *= scale;
    let sky_blend = if view_state.opaque_environment() {
        0.0
    } else {
        projection_transition
    };
    let (inverse_projection, up_and_horizon) = if sky_blend > 0.0 {
        let owned = cached_globe_camera
            .is_none()
            .then(|| crate::render::projection::globe_camera_for_view(view_state).ok())
            .flatten();
        cached_globe_camera
            .or(owned.as_ref())
            .and_then(|camera| {
                let mut inv: [[f32; 4]; 4] = camera.inverse_projection().cast::<f32>()?.into();
                let globe_pos = camera.globe_center_in_view().cast::<f32>()?;
                inv[2] = [
                    camera.camera_to_center_distance() as f32,
                    scale as f32,
                    camera.globe_radius_pixels() as f32,
                    sky.atmosphere_blend_at_zoom(zoom),
                ];
                Some((inv, [globe_pos.x, globe_pos.y, globe_pos.z, 0.0]))
            })
            .unwrap_or_else(|| {
                (
                    cgmath::Matrix4::from_scale(1.0).into(),
                    [0.0, 0.0, 1.0, 0.0],
                )
            })
    } else {
        (
            cgmath::Matrix4::from_scale(1.0).into(),
            [0.0, 0.0, 1.0, 0.0],
        )
    };
    SkyLayerMetadata {
        sky_color: colors.sky,
        horizon_color: colors.horizon,
        horizon: horizon.to_shader(),
        blend: [
            (f64::from(colors.sky_horizon_blend) * height / 2.0) as f32,
            sky_blend,
            height as f32,
            if view_state.has_external_view() {
                1.0e-8
            } else {
                0.0
            },
        ],
        inverse_projection,
        up_and_horizon,
    }
}
