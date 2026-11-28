//! Camera velocity and displacement gating for speculative coarse `z - 3` vector fallbacks.

use crate::{
    render::{tile_view_pattern::DEFAULT_TILE_SIZE, view_state::ViewState},
    tcs::world::World,
};

#[derive(Default)]
struct FallbackMotionTracker {
    prev_norm_xy: Option<[f64; 2]>,
    prev_zoom: f64,
    anchor_norm_xy: Option<[f64; 2]>,
    anchor_zoom: f64,
}

fn wrapped_norm_distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    let raw_dx = (a[0] - b[0]).rem_euclid(1.0);
    let dx = raw_dx.min(1.0 - raw_dx);
    let dy = a[1] - b[1];
    dx.hypot(dy)
}

/// Returns `true` only during active velocity lookahead or rapid/sustained pan/zoom motion so
/// gentle continuous idle micro-jitter does not spawn coarse `z - 3` tiles.
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
    let inv_viewport = 1.0 / viewport_norm.max(1e-9);

    let tracker = world.resources.get_or_init_mut::<FallbackMotionTracker>();
    let triggered =
        if let (Some(prev), Some(anchor)) = (tracker.prev_norm_xy, tracker.anchor_norm_xy) {
            let step_pan = wrapped_norm_distance(norm_xy, prev) * inv_viewport;
            let step_dz = (zoom - tracker.prev_zoom).abs();
            if step_pan <= 1e-4 && step_dz <= 1e-4 {
                tracker.anchor_norm_xy = Some(norm_xy);
                tracker.anchor_zoom = zoom;
                false
            } else {
                let net_pan_viewports = wrapped_norm_distance(norm_xy, anchor) * inv_viewport;
                let net_dz = (zoom - tracker.anchor_zoom).abs();
                step_pan > 1.15 || step_dz > 0.12 || net_pan_viewports > 0.15 || net_dz > 0.12
            }
        } else {
            tracker.anchor_norm_xy = Some(norm_xy);
            tracker.anchor_zoom = zoom;
            false
        };
    tracker.prev_norm_xy = Some(norm_xy);
    tracker.prev_zoom = zoom;

    has_ahead || triggered
}

#[cfg(test)]
mod tests {
    use super::should_request_coarse_fallback;
    use crate::{
        coords::{LatLon, WorldCoords, Zoom},
        render::view_state::ViewState,
        tcs::world::World,
        window::PhysicalSize,
    };

    fn view_at(lng: f64, zoom_val: f64) -> ViewState {
        let zoom = Zoom::new(zoom_val);
        ViewState::new(
            PhysicalSize::new(800, 600).expect("valid"),
            WorldCoords::from_lat_lon(LatLon::new(35.6812, lng), zoom),
            zoom,
            cgmath::Deg(0.0),
            cgmath::Deg(45.0),
        )
    }

    #[test]
    fn cumulative_pan_triggers_coarse_fallback_while_jitter_and_stop_do_not() {
        let mut world = World::default();
        // Cold boot -> false
        let base = view_at(139.7671, 14.0);
        assert!(!should_request_coarse_fallback(&mut world, &base, false));

        // Micro-jitter (< 0.02 viewports) around anchor -> false
        for d in [0.0001, -0.0001, 0.00015, 0.0] {
            let jitter = view_at(139.7671 + d, 14.0);
            assert!(!should_request_coarse_fallback(&mut world, &jitter, false));
        }

        // Smooth 60 fps pan (~0.05 viewports per frame) -> triggers once cumulative net pan > 0.15
        let mut saw_trigger = false;
        for step in 1..=8 {
            let panned = view_at(139.7671 - f64::from(step) * 0.0025, 14.0);
            if should_request_coarse_fallback(&mut world, &panned, false) {
                saw_trigger = true;
                break;
            }
        }
        assert!(saw_trigger, "smooth pan should trigger cumulative fallback");

        // Stopping resets anchor -> false again
        let stopped = view_at(139.7500, 14.0);
        let _ = should_request_coarse_fallback(&mut world, &stopped, false);
        assert!(!should_request_coarse_fallback(&mut world, &stopped, false));
    }
}
