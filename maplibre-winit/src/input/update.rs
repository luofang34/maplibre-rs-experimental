//! Per-frame state application and predictive prefetching for [`InputController`].

use std::time::Duration;

use cgmath::{InnerSpace, Rad, Vector2};
use maplibre::{
    context::MapContext,
    terrain::interaction::{clamp_pitch_above_terrain_keeping_zoom, keep_camera_above_terrain},
};

use super::{InputController, UpdateState};

impl UpdateState for InputController {
    fn update_state(&mut self, map_context: &mut MapContext, dt: Duration) {
        self.navigation_state.ensure_initialized(map_context);
        #[cfg(target_arch = "wasm32")]
        {
            let (safari_pinch, safari_rotate_deg) = crate::web::take_safari_trackpad_gesture();
            if safari_pinch.abs() > 1e-5 {
                self.navigation_state.on_manual_pan_or_zoom();
                self.zoom_handler.process_web_pinch_wheel(safari_pinch);
            }
            if safari_rotate_deg.abs() > 1e-4 {
                self.navigation_state
                    .process_trackpad_rotate(safari_rotate_deg);
            }
        }
        let initial_pitch = map_context.view_state.camera().get_pitch().0;
        let pos_before = map_context.view_state.camera().position();
        let zoom_before = map_context.view_state.zoom().value();
        let before = view_snapshot(map_context);
        if let Some(hud) = map_context
            .world
            .resources
            .get::<maplibre::hud::NavigationHudState>()
        {
            self.zoom_handler.set_sensitivity(hud.zoom_sensitivity);
        }
        let hud_before = map_context
            .world
            .resources
            .get::<maplibre::hud::NavigationHudState>()
            .cloned();

        self.pan_handler.update_state(map_context, dt);
        self.pinch_handler
            .update_state_with_nav(map_context, dt, &mut self.navigation_state);
        self.zoom_handler
            .update_state_with_nav(map_context, dt, Some(&mut self.navigation_state));
        self.camera_handler
            .update_state_with_nav(&mut map_context.view_state, &mut self.navigation_state);
        self.shift_handler
            .update_state_with_nav(map_context, dt, &mut self.navigation_state);
        self.navigation_state.update_frame(map_context, dt);
        self.query_handler.update_state(map_context, dt);
        self.debug_handler.update_state(map_context, dt);

        self.last_known_width = map_context.view_state.width();
        self.last_known_height = map_context.view_state.height();
        self.last_known_zoom = map_context.view_state.zoom().value();
        self.last_known_bearing = map_context.view_state.camera().get_bearing();

        let before_pitch = map_context.view_state.camera().get_pitch().0;
        let handlers_moved = view_snapshot(map_context) != before;

        // Only a view the handlers moved is lifted out of the terrain, as GL JS applies its
        // camera update after a handler changed the transform; elevation data arriving under a
        // resting camera leaves it where it is.
        if handlers_moved {
            let zoom_after_handlers = map_context.view_state.zoom().value();
            let pitch_after_handlers = before_pitch.to_degrees();
            let MapContext {
                style,
                view_state,
                world,
                ..
            } = map_context;
            // Ground `center_elevation` when Auto-Tilt is active or when the gesture started
            // before DEM tiles arrived (`center_elevation == 0`), so terrain clearance is
            // measured relative to the actual terrain under `center` rather than a stale or
            // sea-level reference.
            if style.terrain.is_some()
                && ((self.navigation_state.three_dimensional && self.navigation_state.auto_tilt)
                    || view_state.center_elevation().abs() <= f64::EPSILON
                    || !view_state.center_elevation_frozen())
            {
                let was_frozen = view_state.center_elevation_frozen();
                if was_frozen {
                    view_state.thaw_center_elevation();
                }
                maplibre::terrain::elevation::ground_center(view_state, world);
                if was_frozen {
                    view_state.freeze_center_elevation();
                }
            }
            let pitch_clamped = clamp_pitch_above_terrain_keeping_zoom(style, view_state, world);
            let lifted = keep_camera_above_terrain(style, view_state, world);
            if pitch_clamped || lifted {
                self.navigation_state.cancel_pitch_animation();
                if (view_state.camera().get_pitch().0 - initial_pitch).abs() < 2e-3 {
                    view_state.camera_mut().set_pitch(Rad(initial_pitch));
                }
            }
            let zoom_final = view_state.zoom().value();
            let pitch_final = view_state.camera().get_pitch().0.to_degrees();
            let center_elev = view_state.center_elevation();
            if view_snapshot(map_context) != before {
                tracing::debug!(
                    ?zoom_after_handlers,
                    ?pitch_after_handlers,
                    ?zoom_final,
                    ?pitch_final,
                    ?center_elev,
                    pitch_clamped,
                    lifted,
                    "camera frame update"
                );
            }
        }
        let camera_moved = view_snapshot(map_context) != before;
        update_velocity_prefetch_view(
            &mut self.smoothed_pan_velocity,
            pos_before,
            zoom_before,
            dt,
            map_context,
        );
        let hud_changed = map_context
            .world
            .resources
            .get::<maplibre::hud::NavigationHudState>()
            != hud_before.as_ref();
        self.moved_view = camera_moved
            || hud_changed
            || self.navigation_state.is_animating()
            || self.shift_handler.is_active();
    }
}

/// Projects a velocity-predictive [`maplibre::render::xr::PrefetchView`] ahead of the camera
/// while panning or edge-scrolling so incoming tiles are fetched before entering the screen.
fn update_velocity_prefetch_view(
    smoothed_vel: &mut Vector2<f64>,
    pos_before: cgmath::Point2<f64>,
    zoom_before: f64,
    dt: Duration,
    map_context: &mut MapContext,
) {
    if map_context.view_state.has_external_view() {
        return;
    }
    let zoom_after = map_context.view_state.zoom().value();
    let pos_after = map_context.view_state.camera().position();
    let dt_sec = dt.as_secs_f64();
    let world_size = maplibre::coords::TILE_SIZE * 2_f64.powf(zoom_after);
    if dt_sec > 1e-4 && (zoom_after - zoom_before).abs() < 0.05 {
        let mut dx = pos_after.x - pos_before.x;
        if dx > world_size * 0.5 {
            dx -= world_size;
        } else if dx < -world_size * 0.5 {
            dx += world_size;
        }
        let dy = pos_after.y - pos_before.y;
        let inst_vel = Vector2::new(dx, dy) / dt_sec.max(1.0 / 240.0);
        let alpha = (dt_sec * 14.0).clamp(0.0, 1.0);
        *smoothed_vel = *smoothed_vel * (1.0 - alpha) + inst_vel * alpha;
    } else {
        *smoothed_vel *= 0.5;
    }
    let speed = smoothed_vel.magnitude();
    if speed > 25.0 {
        let view = &map_context.view_state;
        let max_reach = view.width().max(view.height()) * 0.9;
        let mut lead = *smoothed_vel * 0.80;
        if lead.magnitude() > max_reach {
            lead *= max_reach / lead.magnitude();
        }
        let eye = view.eye_position();
        let to_nadir = Vector2::new(eye.x - pos_after.x, eye.y - pos_after.y);
        if to_nadir.magnitude() > 1.0 {
            let align = (*smoothed_vel / speed).dot(to_nadir / to_nadir.magnitude());
            lead += to_nadir * (0.55 * align.max(0.0));
        }
        let mut ahead = view.clone();
        ahead.camera_mut().move_to(cgmath::Point2::new(
            (pos_after.x + lead.x).rem_euclid(world_size),
            (pos_after.y + lead.y).clamp(0.0, world_size),
        ));
        map_context
            .world
            .resources
            .get_or_init_mut::<maplibre::render::xr::PrefetchView>()
            .view_state = Some(ahead);
    } else if let Some(prefetch) = map_context
        .world
        .resources
        .get_mut::<maplibre::render::xr::PrefetchView>()
    {
        if prefetch.placement.is_none() {
            prefetch.view_state = None;
        }
    }
}

/// The parts of the view the input handlers change.
fn view_snapshot(map_context: &MapContext) -> impl PartialEq + use<> {
    let camera = map_context.view_state.camera();
    let position = camera.position();
    (
        Vector2::new(position.x, position.y),
        camera.get_pitch().0,
        camera.get_bearing().0,
        map_context.view_state.zoom().value(),
        map_context.view_state.globe_pose(),
        *map_context.view_state.edge_insets(),
    )
}
