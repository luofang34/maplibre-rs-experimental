//! Continuous keyboard pan (`WASD` / arrows) and bearing orbit (`Q` / `E`) handler.

use std::time::Duration;

use cgmath::{Deg, InnerSpace, Rad, Vector2, Zero};

use super::{
    events::{ButtonState, NavKey},
    navigation_state::{set_pitch_keeping_center, NavigationState},
    projection::{center_pixel, pan_globe_by_pixels, pan_plane_by_pixels},
    UpdateState,
};
use crate::context::MapContext;

pub(crate) struct ShiftHandler {
    camera_translate: Vector2<f64>,
    held_up: bool,
    held_down: bool,
    held_left: bool,
    held_right: bool,
    held_orbit_ccw: bool,
    held_orbit_cw: bool,
    speed: f64,
    sensitivity: f64,
}

impl UpdateState for ShiftHandler {
    fn update_state(&mut self, map_context: &mut MapContext, dt: Duration) {
        let mut dummy_nav = NavigationState::default();
        self.update_state_with_nav(map_context, dt, &mut dummy_nav);
    }
}

impl ShiftHandler {
    pub(crate) fn new(speed: f64, sensitivity: f64) -> Self {
        Self {
            camera_translate: Vector2::zero(),
            held_up: false,
            held_down: false,
            held_left: false,
            held_right: false,
            held_orbit_ccw: false,
            held_orbit_cw: false,
            speed,
            sensitivity,
        }
    }

    /// Whether any directional or orbit key is currently held down or decaying.
    pub(crate) fn is_active(&self) -> bool {
        self.held_up
            || self.held_down
            || self.held_left
            || self.held_right
            || self.held_orbit_ccw
            || self.held_orbit_cw
            || self.camera_translate.magnitude2() > 0.01
    }

    pub(crate) fn update_state_with_nav(
        &mut self,
        state: &mut MapContext,
        dt: Duration,
        nav: &mut NavigationState,
    ) {
        self.update_parts_with_nav(&state.style, &mut state.view_state, dt, nav);
    }

    pub(crate) fn update_parts_with_nav(
        &mut self,
        style: &crate::style::Style,
        view_state: &mut crate::render::view_state::ViewState,
        dt: Duration,
        nav: &mut NavigationState,
    ) {
        let dt_secs = dt.as_secs_f64();
        let boost = if nav.shift_held { 1.85 } else { 1.0 };

        // Continuous held-key velocity (panning or Alt+Arrow orbiting/tilting).
        let mut dir = Vector2::zero();
        if self.held_up {
            dir.y -= 1.0;
        }
        if self.held_down {
            dir.y += 1.0;
        }
        if self.held_left {
            dir.x -= 1.0;
        }
        if self.held_right {
            dir.x += 1.0;
        }

        if nav.alt_held && (dir.x != 0.0 || dir.y != 0.0) {
            if dir.x != 0.0 && !nav.north_locked {
                nav.on_manual_bearing();
                let current = view_state.camera().get_bearing();
                let delta = Rad::from(Deg(dir.x * 95.0 * boost * dt_secs));
                view_state.camera_mut().set_bearing(current + delta);
            }
            if dir.y != 0.0 && nav.three_dimensional {
                nav.on_manual_tilt();
                let max_pitch: f64 = view_state.camera().max_pitch().0.to_degrees();
                let current: f64 = view_state.camera().get_pitch().0.to_degrees();
                let dy: f64 = dir.y;
                let next = (current - dy * 55.0 * boost * dt_secs).clamp(0.0, max_pitch);
                set_pitch_keeping_center(view_state, Deg(next));
            }
            self.camera_translate = Vector2::zero();
        } else if dir.magnitude2() > 0.0 {
            nav.on_manual_pan_or_zoom();
            let unit = dir.normalize();
            let continuous_px = unit * (6.2 * self.sensitivity * boost * dt_secs);
            let center = center_pixel(view_state);
            let plane = view_state.center_elevation();
            if !pan_globe_by_pixels(style, view_state, center, -continuous_px) {
                pan_plane_by_pixels(view_state, center, -continuous_px, plane);
            }
            self.camera_translate = Vector2::zero();
        } else {
            let decay = (dt_secs / self.speed).min(1.0);
            let delta = self.camera_translate * decay;
            self.camera_translate -= delta;
            if self.camera_translate.magnitude2() <= 0.01 {
                self.camera_translate = Vector2::zero();
            }
            if !delta.is_zero() {
                let center = center_pixel(view_state);
                let plane = view_state.center_elevation();
                if !pan_globe_by_pixels(style, view_state, center, -delta) {
                    pan_plane_by_pixels(view_state, center, -delta, plane);
                }
            }
        }

        // Continuous Q / E bearing rotation.
        let mut orbit_dir = 0.0;
        if self.held_orbit_ccw {
            orbit_dir -= 1.0;
        }
        if self.held_orbit_cw {
            orbit_dir += 1.0;
        }
        if orbit_dir != 0.0 && !nav.north_locked {
            nav.on_manual_bearing();
            let current = view_state.camera().get_bearing();
            let delta = Rad::from(Deg(orbit_dir * 95.0 * boost * dt_secs));
            view_state.camera_mut().set_bearing(current + delta);
        }
    }

    pub(crate) fn process_key(&mut self, key: NavKey, state: ButtonState) -> bool {
        let pressed = state == ButtonState::Pressed;
        let amount = if pressed {
            10.0 * self.sensitivity
        } else {
            0.0
        };
        match key {
            NavKey::Up => {
                self.held_up = pressed;
                self.camera_translate.y -= amount;
                true
            }
            NavKey::Down => {
                self.held_down = pressed;
                self.camera_translate.y += amount;
                true
            }
            NavKey::Left => {
                self.held_left = pressed;
                self.camera_translate.x -= amount;
                true
            }
            NavKey::Right => {
                self.held_right = pressed;
                self.camera_translate.x += amount;
                true
            }
            NavKey::OrbitCcw => {
                self.held_orbit_ccw = pressed;
                true
            }
            NavKey::OrbitCw => {
                self.held_orbit_cw = pressed;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_idle_gap_clamps_decay_fraction_without_overshoot_or_sign_flip() {
        let mut handler = ShiftHandler::new(0.2, 100.0);
        assert!(handler.process_key(NavKey::Right, ButtonState::Pressed));
        let initial_x = handler.camera_translate.x;
        assert!(initial_x > 0.0);

        let factor = (Duration::from_secs(60).as_secs_f64() / handler.speed).min(1.0);
        let delta = handler.camera_translate * factor;
        handler.camera_translate -= delta;
        assert_eq!(delta.x, initial_x);
        assert_eq!(handler.camera_translate.x, 0.0);
    }
}
