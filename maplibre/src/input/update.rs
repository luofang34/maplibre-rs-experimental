//! Per-frame state application and predictive prefetching for [`InputController`].

use std::time::Duration;

use cgmath::{InnerSpace, Rad, Vector2};
use instant::Instant;

use super::{InputController, UpdateState};
use crate::{
    context::MapContext,
    coords::TILE_SIZE,
    hud::NavigationHudState,
    render::{view_state::ViewState, xr::PrefetchView},
    style::Style,
    tcs::world::World,
    terrain::interaction::{clamp_pitch_above_terrain_keeping_zoom, keep_camera_above_terrain},
};

impl UpdateState for InputController {
    fn update_state(&mut self, map_context: &mut MapContext, dt: Duration) {
        self.update_state_at(map_context, dt, Instant::now());
    }
}

impl InputController {
    /// Advances accumulated gesture and animation state on `map_context` using an explicit `now`.
    pub fn update_state_at(&mut self, map_context: &mut MapContext, dt: Duration, now: Instant) {
        self.navigation_state.ensure_initialized(map_context);
        let hud_before = map_context
            .world
            .resources
            .get::<NavigationHudState>()
            .cloned();
        if let Some(hud) = &hud_before {
            self.zoom_handler.set_sensitivity(hud.zoom_sensitivity);
        }

        let camera_moved = self.apply_handlers_at(
            &map_context.style,
            &mut map_context.view_state,
            &mut map_context.world,
            dt,
            now,
            false,
        );
        self.navigation_state.update_frame(map_context, dt);

        self.last_known_width = map_context.view_state.width();
        self.last_known_height = map_context.view_state.height();
        self.last_known_zoom = map_context.view_state.zoom().value();
        self.last_known_bearing = map_context.view_state.camera().get_bearing();

        let hud_changed =
            map_context.world.resources.get::<NavigationHudState>() != hud_before.as_ref();
        self.moved_view = camera_moved
            || hud_changed
            || self.pan_handler.is_coasting()
            || self.pinch_handler.is_coasting()
            || self.navigation_state.is_animating()
            || self.shift_handler.is_active();
    }

    /// Advances accumulated gesture and animation state directly on a [`Style`] + [`ViewState`]
    /// without requiring a GPU [`crate::render::Renderer`] or [`MapContext`].
    pub fn update_view(&mut self, style: &Style, view_state: &mut ViewState, dt: Duration) {
        self.update_view_at(style, view_state, dt, Instant::now());
    }

    /// Advances accumulated gesture and animation state directly on a [`Style`] + [`ViewState`]
    /// at an explicit `now` timestamp (ideal for deterministic unit tests and headless hosts).
    pub fn update_view_at(
        &mut self,
        style: &Style,
        view_state: &mut ViewState,
        dt: Duration,
        now: Instant,
    ) {
        let mut world = World::default();
        let camera_moved = self.apply_handlers_at(style, view_state, &mut world, dt, now, true);
        self.last_known_width = view_state.width();
        self.last_known_height = view_state.height();
        self.last_known_zoom = view_state.zoom().value();
        self.last_known_bearing = view_state.camera().get_bearing();
        self.moved_view = camera_moved
            || self.pan_handler.is_coasting()
            || self.pinch_handler.is_coasting()
            || self.navigation_state.is_animating()
            || self.shift_handler.is_active();
    }

    fn apply_handlers_at(
        &mut self,
        style: &Style,
        view_state: &mut ViewState,
        world: &mut World,
        dt: Duration,
        now: Instant,
        advance_nav_directly: bool,
    ) -> bool {
        let initial_pitch = view_state.camera().get_pitch().0;
        let pos_before = view_state.camera().position();
        let zoom_before = view_state.zoom().value();
        let before = view_snapshot(view_state);

        self.pan_handler
            .update_parts_at(style, view_state, world, now);
        self.pinch_handler
            .update_parts_with_nav(style, view_state, dt, &mut self.navigation_state);
        self.zoom_handler.update_parts_with_nav_at(
            style,
            view_state,
            world,
            dt,
            Some(&mut self.navigation_state),
            now,
        );
        self.camera_handler
            .update_state_with_nav(view_state, &mut self.navigation_state);
        self.shift_handler
            .update_parts_with_nav(style, view_state, dt, &mut self.navigation_state);
        if advance_nav_directly {
            self.navigation_state.advance(style, view_state, dt);
        }
        self.query_handler.update_parts(style, view_state, world);
        self.debug_handler.update_parts(view_state, dt);

        let before_pitch = view_state.camera().get_pitch().0;
        let handlers_moved = view_snapshot(view_state) != before;

        if handlers_moved {
            let zoom_after_handlers = view_state.zoom().value();
            let pitch_after_handlers = before_pitch.to_degrees();
            if style.terrain.is_some()
                && ((self.navigation_state.three_dimensional && self.navigation_state.auto_tilt)
                    || view_state.center_elevation().abs() <= f64::EPSILON
                    || !view_state.center_elevation_frozen())
            {
                let was_frozen = view_state.center_elevation_frozen();
                if was_frozen {
                    view_state.thaw_center_elevation();
                }
                crate::terrain::elevation::ground_center(view_state, world);
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
            if view_snapshot(view_state) != before {
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

        let camera_moved = view_snapshot(view_state) != before;
        update_velocity_prefetch_view(
            &mut self.smoothed_pan_velocity,
            pos_before,
            zoom_before,
            dt,
            view_state,
            world,
        );
        camera_moved
    }
}

fn update_velocity_prefetch_view(
    smoothed_vel: &mut Vector2<f64>,
    pos_before: cgmath::Point2<f64>,
    zoom_before: f64,
    dt: Duration,
    view: &ViewState,
    world: &mut World,
) {
    if view.has_external_view() {
        return;
    }
    let zoom_after = view.zoom().value();
    let pos_after = view.camera().position();
    let dt_sec = dt.as_secs_f64();
    let world_size = TILE_SIZE * 2_f64.powf(zoom_after);
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
        world.resources.get_or_init_mut::<PrefetchView>().view_state = Some(ahead);
    } else if let Some(prefetch) = world.resources.get_mut::<PrefetchView>() {
        if prefetch.placement.is_none() {
            prefetch.view_state = None;
        }
    }
}

fn view_snapshot(view_state: &ViewState) -> impl PartialEq + use<> {
    let camera = view_state.camera();
    let position = camera.position();
    (
        Vector2::new(position.x, position.y),
        camera.get_pitch().0,
        camera.get_bearing().0,
        view_state.zoom().value(),
        view_state.globe_pose(),
        *view_state.edge_insets(),
    )
}
