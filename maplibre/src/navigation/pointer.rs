//! Pointer input: HUD hover, press and release, double-click zoom, and trackpad rotation.

use std::time::Duration;

use cgmath::{InnerSpace, Rad, Vector2};
use instant::Instant;

use super::NavigationState;
use crate::{
    hud::{HudButton, NavigationHudState, HUD_HEIGHT, HUD_WIDTH},
    render::view_state::ViewState,
};

impl NavigationState {
    /// Whether the given logical pixel lies inside the floating HUD capsule.
    pub fn is_over_hud(&self, view_state: &ViewState, cursor: Vector2<f64>) -> bool {
        if !self.show_hud {
            return false;
        }
        let origin = NavigationHudState::panel_origin(
            view_state.width(),
            view_state.height(),
            view_state.edge_insets(),
        );
        let rel = cursor - origin;
        rel.x >= -2.0 && rel.x <= HUD_WIDTH + 2.0 && rel.y >= -2.0 && rel.y <= HUD_HEIGHT + 2.0
    }

    /// Updates cursor hover and active HUD compass drag / selection rectangle.
    /// Returns `true` if the HUD consumed the cursor movement.
    pub fn process_cursor_moved(
        &mut self,
        position: Vector2<f64>,
        view_width: f64,
        view_height: f64,
    ) -> bool {
        self.cursor_position = Some(position);

        if let Some((start, _)) = self.selection_drag {
            self.selection_drag = Some((start, position));
        }

        if self.pressed_button == Some(HudButton::Compass) {
            if let Some(start) = self.compass_press_start {
                if (position - start).magnitude() > 3.5 {
                    self.compass_dragged = true;
                }
            }
            return true;
        }

        if self.show_hud && view_width > 0.0 && view_height > 0.0 {
            let insets = crate::render::camera::EdgeInsets::default();
            self.hovered_button =
                NavigationHudState::hit_test(position, view_width, view_height, &insets);
        } else {
            self.hovered_button = None;
        }

        self.pressed_button.is_some()
    }

    /// Handles a primary (left) press or touch start at `position`.
    /// Returns `true` if the floating HUD intercepted the press.
    pub fn process_hud_press(
        &mut self,
        position: Vector2<f64>,
        view_width: f64,
        view_height: f64,
    ) -> bool {
        if !self.show_hud || view_width <= 0.0 || view_height <= 0.0 {
            return false;
        }
        let insets = crate::render::camera::EdgeInsets::default();
        if let Some(button) =
            NavigationHudState::hit_test(position, view_width, view_height, &insets)
        {
            self.pressed_button = Some(button);
            self.hovered_button = Some(button);
            if button == HudButton::Compass {
                self.compass_press_start = Some(position);
                self.compass_drag_last = Some(position);
                self.compass_dragged = false;
            }
            return true;
        }
        false
    }

    /// Handles releasing a primary (left) press or touch end.
    /// Returns `true` if a HUD button was active and handled the release.
    pub fn process_hud_release(&mut self, current_zoom: f64, current_bearing: Rad<f64>) -> bool {
        let Some(button) = self.pressed_button.take() else {
            return false;
        };
        let was_compass_drag = self.compass_dragged;
        self.compass_press_start = None;
        self.compass_drag_last = None;
        self.compass_dragged = false;

        match button {
            HudButton::Compass => {
                if !was_compass_drag {
                    self.trigger_compass(current_bearing);
                }
            }
            HudButton::Mode3D => self.cycle_3d_mode(current_zoom),
            HudButton::ZoomIn => {
                self.fly_home = false;
                self.pending_smooth_zoom += 1.0;
            }
            HudButton::ZoomOut => {
                self.fly_home = false;
                self.pending_smooth_zoom -= 1.0;
            }
            HudButton::Home => self.trigger_home(),
            HudButton::PointerMode => self.toggle_pointer_profile(),
        }
        true
    }

    /// Checks whether a primary press at `position` is a double-click / double-tap, and if so,
    /// queues a smooth `+1.0` zoom (`-1.0` when `Shift` or `Alt` is held).
    pub fn check_double_click(&mut self, now: Instant, position: Vector2<f64>) -> bool {
        const DOUBLE_CLICK_MAX_DELAY: Duration = Duration::from_millis(320);
        const DOUBLE_CLICK_MAX_DIST: f64 = 24.0;

        if let Some((last_time, last_pos)) = self.last_primary_click {
            if now.saturating_duration_since(last_time) <= DOUBLE_CLICK_MAX_DELAY
                && (position - last_pos).magnitude() <= DOUBLE_CLICK_MAX_DIST
            {
                self.last_primary_click = None;
                self.fly_home = false;
                let delta = if self.shift_held || self.alt_held {
                    -1.0
                } else {
                    1.0
                };
                self.pending_smooth_zoom += delta;
                return true;
            }
        }
        self.last_primary_click = Some((now, position));
        false
    }
}
