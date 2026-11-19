//! Camera velocity and displacement gating for speculative coarse `z - 3` vector fallbacks.

use crate::{
    render::{tile_view_pattern::DEFAULT_TILE_SIZE, view_state::ViewState},
    tcs::world::World,
};

#[derive(Default)]
struct FallbackMotionTracker {
    prev_norm_xy: Option<[f64; 2]>,
    prev_zoom: f64,
}

/// Returns `true` only during active velocity lookahead or rapid pan/zoom jumps so gentle continuous
/// camera motion (e.g. slow globe rotation or micro-jitter) does not spawn coarse `z - 3` tiles.
pub(super) fn should_request_coarse_fallback(
    world: &mut World,
    view_state: &ViewState,
    has_ahead: bool,
) -> bool {
    let zoom = view_state.zoom().value();
    let world_size = (DEFAULT_TILE_SIZE * 2.0_f64.powf(zoom)).max(1.0);
    let pos = view_state.camera().position();
    let norm_xy = [pos.x / world_size, pos.y / world_size];
    let viewport_norm = (view_state.width().max(view_state.height()).max(1.0)) / world_size;

    let tracker = world.resources.get_or_init_mut::<FallbackMotionTracker>();
    let triggered = if let Some(prev) = tracker.prev_norm_xy {
        let raw_dx = (norm_xy[0] - prev[0]).rem_euclid(1.0);
        let dx = raw_dx.min(1.0 - raw_dx);
        let dy = norm_xy[1] - prev[1];
        let pan_viewports = dx.hypot(dy) / viewport_norm.max(1e-9);
        let dz = (zoom - tracker.prev_zoom).abs();
        pan_viewports > 1.15 || dz > 0.12
    } else {
        false
    };
    tracker.prev_norm_xy = Some(norm_xy);
    tracker.prev_zoom = zoom;

    has_ahead || triggered
}
