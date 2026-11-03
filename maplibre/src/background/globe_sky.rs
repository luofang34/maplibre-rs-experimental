//! The sky behind a globe seen from an eye in or above the atmosphere: an analytic gradient
//! from the horizon colour at the eye's sea-level horizon to the sky colour above it.
//!
//! Unlike the atmosphere's scattering, the gradient follows only the elevation angle of each
//! pixel's ray above the eye's local horizon, so it does not change with heading, sun or
//! altitude: what an avionics synthetic-vision display needs for constant attitude cueing.

use cgmath::{InnerSpace, Vector3};

use crate::{
    render::{projection::globe_camera_for_view, shaders::SkyLayerMetadata, view_state::ViewState},
    style::sky::SkySpecification,
};

/// Smallest sky-horizon blend, as a share of the half field of view, so the horizon colour
/// always shows as a band rather than a line.
const MIN_BLEND: f32 = 0.05;
/// Smallest half field of view the blend is measured against, in radians.
const MIN_HALF_FIELD_OF_VIEW: f32 = 0.25;

/// Whether the sky is drawn as [`globe_sky_metadata`] does: on a full globe without
/// atmosphere, where the host asked for an opaque environment.
pub fn draws_globe_sky(
    view_state: &ViewState,
    projection_transition: f32,
    atmosphere_blend: f32,
) -> bool {
    projection_transition >= 1.0
        && atmosphere_blend <= 0.0
        && view_state.opaque_environment_requested()
}

/// The globe sky's values for the frame, or `None` when the view has no globe camera.
///
/// The sky is drawn at depth zero, so under reversed-Z it fills only pixels no globe
/// background or terrain covered: above the limb at any altitude, with no gap and without
/// hiding the ground colour below it.
pub fn globe_sky_metadata(
    sky: &SkySpecification,
    view_state: &ViewState,
) -> Option<SkyLayerMetadata> {
    let camera = globe_camera_for_view(view_state).ok()?;
    let colors = sky.colors_at(view_state.zoom().value());
    let inverse_projection = camera.inverse_projection().cast::<f32>()?;
    let eye = camera.camera_position();
    let radius = eye.magnitude();
    let up_world = if radius > 1e-9 {
        eye / radius
    } else {
        Vector3::new(0.0, 0.0, 1.0)
    };
    let up_view = camera
        .world_direction_to_view(up_world)
        .and_then(|up| up.cast::<f32>())
        .unwrap_or(Vector3::new(0.0, 1.0, 0.0));
    // The tangent from an eye at `radius` (unit globe) meets the sphere this far below the
    // eye's horizontal; inside the sphere the eye's own horizontal is the horizon.
    let horizon_elevation = -((1.0 / radius.max(1.0)).clamp(0.0, 1.0).acos() as f32);
    let half_field_of_view =
        (camera.field_of_view_degrees().to_radians() as f32 * 0.5).max(MIN_HALF_FIELD_OF_VIEW);
    let blend_angle = colors.sky_horizon_blend.max(MIN_BLEND) * half_field_of_view;
    Some(SkyLayerMetadata {
        sky_color: colors.sky,
        horizon_color: colors.horizon,
        horizon: view_state.horizon_line().to_shader(),
        blend: [blend_angle, 0.0, -1.0, 0.0],
        inverse_projection: inverse_projection.into(),
        up_and_horizon: [up_view.x, up_view.y, up_view.z, horizon_elevation],
    })
}
