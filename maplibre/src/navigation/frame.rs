//! Per-frame navigation: compass drag, trackpad rotation, North-Lock, smooth settling,
//! screen-edge scrolling, and the HUD state the GPU overlay draws.

use std::time::Duration;

use cgmath::{Deg, InnerSpace, Rad, Vector2};

use super::{
    camera::{
        normalize_angle_signed, rotate_bearing_around_cursor, set_bearing_keeping_center,
        set_pitch_keeping_center,
    },
    projection::{
        center_pixel, pan_globe_by_pixels, pan_plane_by_pixels, zoom_globe_around_pixel_with_pitch,
    },
    NavigationState, PointerProfile,
};
use crate::{
    context::MapContext,
    coords::{Zoom, TILE_SIZE},
    hud::{HudButton, NavigationHudState},
    render::view_state::ViewState,
    style::Style,
};

/// Width of the band along each viewport edge that scrolls the map, in logical pixels.
const EDGE_ZONE_PX: f64 = 24.0;

impl NavigationState {
    /// Captures the home camera on the first frame, advances the camera with
    /// [`advance`](Self::advance), and syncs `NavigationHudState` into `world.resources` for
    /// the GPU HUD overlay.
    pub fn update_frame(&mut self, map_context: &mut MapContext, dt: Duration) {
        self.ensure_initialized(map_context);
        self.advance(&map_context.style, &mut map_context.view_state, dt);
        self.sync_hud(map_context);
    }

    /// Moves the camera by `dt` of HUD compass drag, trackpad rotation, North-Lock, smooth
    /// settle animations, and edge scrolling. A host that draws its own controls calls this
    /// each frame while [`wants_frame`](Self::wants_frame) holds.
    pub fn advance(&mut self, style: &Style, view: &mut ViewState, dt: Duration) {
        self.update_hover(view);
        self.apply_compass_drag(view);
        if self.pending_trackpad_rotation.0.abs() > 1e-6 {
            let delta = std::mem::replace(&mut self.pending_trackpad_rotation, Rad(0.0));
            if !self.north_locked {
                rotate_bearing_around_cursor(style, view, self.cursor_position, delta);
            }
        }
        if self.north_locked && normalize_angle_signed(view.camera().get_bearing().0).abs() > 1e-4 {
            set_bearing_keeping_center(view, Rad(0.0));
        }
        let alpha = (1.0 - (-dt.as_secs_f64() * 14.0).exp()).clamp(0.05, 1.0);
        self.advance_fly_home(style, view, alpha);
        self.advance_bearing(view, alpha);
        self.advance_pitch(view, alpha);
        self.edge_scroll(style, view, dt);
    }

    /// Hit-tests the cursor against the HUD with the viewport's actual size and edge insets.
    fn update_hover(&mut self, view: &ViewState) {
        let Some(cursor) = self.cursor_position else {
            return;
        };
        self.hovered_button = if self.show_hud {
            NavigationHudState::hit_test(cursor, view.width(), view.height(), view.edge_insets())
        } else {
            None
        };
    }

    /// Applies the pointer movement of a HUD compass drag.
    fn apply_compass_drag(&mut self, view: &mut ViewState) {
        if self.pressed_button != Some(HudButton::Compass) || !self.compass_dragged {
            return;
        }
        let (Some(cursor), Some(last)) = (self.cursor_position, self.compass_drag_last) else {
            return;
        };
        self.compass_drag_last = Some(cursor);
        self.drag_compass(view, cursor - last);
    }

    /// Turns the bearing with the horizontal and the pitch with the vertical component of a
    /// compass drag of `delta` logical pixels, for hosts that draw their own compass.
    pub fn drag_compass(&mut self, view: &mut ViewState, delta: Vector2<f64>) {
        if delta.magnitude2() <= f64::EPSILON {
            return;
        }
        if !self.north_locked && delta.x.abs() > 0.0 {
            let bearing = view.camera().get_bearing();
            set_bearing_keeping_center(view, bearing - Rad::from(Deg(delta.x * 0.65)));
            self.on_manual_bearing();
        }
        if delta.y.abs() > 0.0 {
            let max_pitch = view.camera().max_pitch().0.to_degrees();
            let pitch = view.camera().get_pitch().0.to_degrees();
            self.on_manual_tilt();
            set_pitch_keeping_center(view, Deg((pitch - delta.y * 0.5).clamp(0.0, max_pitch)));
        }
    }

    /// Eases zoom and center toward the home camera.
    fn advance_fly_home(&mut self, style: &Style, view: &mut ViewState, alpha: f64) {
        if !self.fly_home {
            return;
        }
        let Some(home) = self.home else {
            self.fly_home = false;
            return;
        };
        let current_zoom = view.zoom().value();
        let next_zoom = current_zoom + (home.zoom - current_zoom) * alpha;
        let center = center_pixel(view);
        // On Mercator views there is no globe to zoom around; `zoom_to` below covers both.
        zoom_globe_around_pixel_with_pitch(style, view, center, Zoom::new(next_zoom), None);
        view.zoom_to(Zoom::new(next_zoom));
        let world_size = TILE_SIZE * 2.0_f64.powf(view.zoom().value());
        let current = view.camera().position();
        let target = home.normalized_center * world_size;
        let next = current + (target - current) * alpha;
        view.camera_mut().move_to(next);
        if (home.zoom - next_zoom).abs() < 0.01 && (target - next).magnitude() < 0.5 {
            view.zoom_to(Zoom::new(home.zoom));
            let world_size = TILE_SIZE * 2.0_f64.powf(home.zoom);
            view.camera_mut()
                .move_to(home.normalized_center * world_size);
            self.fly_home = false;
        }
    }

    /// Eases the bearing toward its target the short way round.
    fn advance_bearing(&mut self, view: &mut ViewState, alpha: f64) {
        let Some(target) = self.target_bearing_rad else {
            return;
        };
        let current = view.camera().get_bearing().0;
        let diff = normalize_angle_signed(target - current);
        if diff.abs() <= 0.0015 {
            set_bearing_keeping_center(view, Rad(target));
            self.target_bearing_rad = None;
        } else {
            set_bearing_keeping_center(view, Rad(current + diff * alpha));
        }
    }

    /// Eases the pitch toward its target, or back onto the Auto-Tilt curve once terrain
    /// clearance allows, such as after panning off a steep ridge over a valley.
    fn advance_pitch(&mut self, view: &mut ViewState, alpha: f64) {
        let max_pitch = view.camera().max_pitch().0.to_degrees();
        let current = view.camera().get_pitch().0.to_degrees();
        if let Some(target) = self.target_pitch_deg {
            let target = target.clamp(0.0, max_pitch);
            let diff = target - current;
            if diff.abs() <= 0.08 {
                set_pitch_keeping_center(view, Deg(target));
                self.target_pitch_deg = None;
            } else {
                set_pitch_keeping_center(view, Deg(current + diff * alpha));
            }
        } else if self.three_dimensional && self.auto_tilt && !self.fly_home {
            let desired = self
                .pitch_for_zoom(view.zoom().value())
                .clamp(0.0, max_pitch);
            let diff = desired - current;
            if diff.abs() > super::modes::AUTO_TILT_TOLERANCE_DEG {
                set_pitch_keeping_center(view, Deg(current + diff * alpha));
            }
        }
    }

    /// Scrolls the map while the cursor rests near a viewport edge in the RTS profile, or
    /// whenever edge scrolling is enabled.
    fn edge_scroll(&mut self, style: &Style, view: &mut ViewState, dt: Duration) {
        if !(self.pointer_profile == PointerProfile::Rts || self.edge_scroll_override)
            || self.pressed_button.is_some()
            || self.selection_drag.is_some()
        {
            return;
        }
        let Some(cursor) = self.cursor_position else {
            return;
        };
        let (w, h) = (view.width(), view.height());
        if w <= 100.0
            || h <= 100.0
            || !(0.0..=w).contains(&cursor.x)
            || !(0.0..=h).contains(&cursor.y)
            || self.is_over_hud(view, cursor)
        {
            return;
        }
        let velocity = Vector2::new(edge_velocity(cursor.x, w), edge_velocity(cursor.y, h));
        if velocity.x == 0.0 && velocity.y == 0.0 {
            return;
        }
        let boost = if self.shift_held { 1.8 } else { 1.0 };
        let step =
            velocity * (self.edge_scroll_speed_px_per_sec.max(0.0) * boost * dt.as_secs_f64());
        let center = center_pixel(view);
        let plane = view.center_elevation();
        if !pan_globe_by_pixels(style, view, center, -step) {
            pan_plane_by_pixels(view, center, -step, plane);
        }
    }

    /// Publishes the state the GPU HUD overlay draws.
    fn sync_hud(&self, map_context: &mut MapContext) {
        let hud = map_context
            .world
            .resources
            .get_or_init_mut::<NavigationHudState>();
        hud.visible = self.show_hud;
        hud.three_dimensional = self.three_dimensional;
        hud.auto_tilt = self.auto_tilt;
        hud.north_locked = self.north_locked;
        hud.rts_mode = self.pointer_profile == PointerProfile::Rts;
        hud.hovered = self.hovered_button;
        hud.pressed = self.pressed_button;
        hud.selection_box = self.selection_drag.and_then(|(start, end)| {
            ((end - start).magnitude() >= 3.0).then_some([
                start.x.min(end.x) as f32,
                start.y.min(end.y) as f32,
                start.x.max(end.x) as f32,
                start.y.max(end.y) as f32,
            ])
        });
    }
}

/// Edge-scroll velocity along one axis in `-1..=1`, easing in quadratically across the band.
fn edge_velocity(position: f64, extent: f64) -> f64 {
    if position < EDGE_ZONE_PX {
        let t = ((EDGE_ZONE_PX - position) / EDGE_ZONE_PX).clamp(0.0, 1.0);
        -t * t
    } else if position > extent - EDGE_ZONE_PX {
        let t = ((position - (extent - EDGE_ZONE_PX)) / EDGE_ZONE_PX).clamp(0.0, 1.0);
        t * t
    } else {
        0.0
    }
}
