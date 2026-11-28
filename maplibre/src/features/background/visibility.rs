//! Viewport coverage and atmosphere-blend predicates shared by background resource and queue systems.

use super::buffers::world_rows;
use crate::{
    projection::globe::camera::GlobeCameraState,
    render::{resolved_view::ResolvedFrameView, view_state::ViewState},
};

/// Same as [`flat_viewport_covered`], reusing already-computed `horizon` and `world_rows`.
pub(crate) fn flat_viewport_covered_precomputed(
    view_state: &ViewState,
    edges: bool,
    frame: [f32; 2],
    horizon: [f32; 4],
    [row_num, row_den]: [[f32; 4]; 2],
) -> bool {
    if !edges || frame[0] <= 0.0 || frame[1] <= 0.0 {
        return false;
    }
    let logical_h = view_state.height() as f32;
    let scale = logical_h / frame[1].max(1.0);
    for (px, py, nx, ny) in [
        (0.0_f32, 0.0_f32, -1.0_f32, 1.0_f32),
        (frame[0], 0.0, 1.0, 1.0),
        (0.0, frame[1], -1.0, -1.0),
        (frame[0], frame[1], 1.0, -1.0),
    ] {
        let x = px * scale;
        let y = logical_h - py * scale;
        let distance = (y - horizon[1]) * horizon[3] + (x - horizon[0]) * horizon[2];
        if !distance.is_finite() || distance > -1e-3 {
            return false;
        }
        let denom = row_den[0] * nx + row_den[1] * ny + row_den[2];
        if !denom.is_finite() || denom <= 1e-9 {
            return false;
        }
        let row = (row_num[0] * nx + row_num[1] * ny + row_num[2]) / denom;
        if !row.is_finite() || row < row_num[3] + 1e-3 || row > row_den[3] - 1e-3 {
            return false;
        }
    }
    true
}

/// Returns `true` when an unclipped flat quad is guaranteed to pass every `background.fragment.wgsl` check.
pub(crate) fn flat_viewport_covered(view_state: &ViewState, edges: bool, frame: [f32; 2]) -> bool {
    if !edges || frame[0] <= 0.0 || frame[1] <= 0.0 {
        return false;
    }
    let horizon = view_state.horizon_line().to_shader();
    let rows = world_rows(view_state, edges);
    flat_viewport_covered_precomputed(view_state, edges, frame, horizon, rows)
}

pub(super) fn atmosphere_blend_state_with(
    style: &crate::style::Style,
    view_state: &crate::render::view_state::ViewState,
    world: &crate::tcs::world::World,
    transition: f32,
    resolved_camera: Option<&GlobeCameraState>,
) -> (f32, bool) {
    let zoom = view_state.zoom().value();
    let sky_blend = style
        .sky
        .as_ref()
        .map_or(0.0, |sky| sky.atmosphere_blend_at_zoom(zoom));
    let host_blend = f32::from(u8::from(
        world
            .resources
            .get::<crate::render::SceneLighting>()
            .is_some_and(|l| l.atmosphere),
    ));
    let sky_faded = if sky_blend > 0.0 && transition > 0.0 {
        let owned = if resolved_camera.is_none() {
            ResolvedFrameView::globe_camera_for_frame(world, style, view_state).ok()
        } else {
            None
        };
        let altitude_blend = resolved_camera
            .or(owned.as_deref())
            .map_or(0.0, super::altitude::altitude_blend_for_camera);
        sky_blend * altitude_blend
    } else {
        0.0
    };
    let over_globe = sky_faded > 0.0 && host_blend <= 0.0;
    (sky_faded.max(host_blend) * transition, over_globe)
}

fn atmosphere_blend_state(
    style: &crate::style::Style,
    view_state: &crate::render::view_state::ViewState,
    world: &crate::tcs::world::World,
) -> (f32, bool) {
    let zoom = view_state.zoom().value();
    let transition = style.projection.as_ref().map_or(0.0, |specification| {
        specification.projection_type.globe_transition(zoom)
    });
    let resolved_camera = world
        .resources
        .get::<ResolvedFrameView>()
        .filter(|resolved| resolved.matches(style, view_state))
        .and_then(ResolvedFrameView::globe_camera);
    atmosphere_blend_state_with(style, view_state, world, transition, resolved_camera)
}

/// How strongly the atmosphere is drawn this frame: as the style's `sky` blends it, or in full
/// where the host turned it on, faded with the projection's globe transition and camera altitude below 100 km.
pub fn atmosphere_blend(
    style: &crate::style::Style,
    view_state: &crate::render::view_state::ViewState,
    world: &crate::tcs::world::World,
) -> f32 {
    atmosphere_blend_state(style, view_state, world).0
}
