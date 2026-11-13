//! Pointer orbit/tilt and `Alt`+trackpad orbit handler.

use std::time::Duration;

use cgmath::{Deg, MetricSpace, Rad, Vector2};

use super::{
    events::{ButtonState, PointerButton},
    navigation_state::NavigationState,
    UpdateState,
};
use crate::{
    context::MapContext,
    render::view_state::{GlobePose, NavigationMode, ViewState},
};

pub(crate) struct CameraHandler {
    window_position: Option<Vector2<f64>>,
    start_window_position: Option<Vector2<f64>>,
    is_active: bool,
    is_middle: bool,

    start_delta_pitch: Option<Rad<f64>>,
    start_delta_roll: Option<Rad<f64>>,
    /// The free-globe pose the drag started from.
    start_pose: Option<GlobePose>,
    /// Accumulated 2D scroll delta from `Alt` + two-finger trackpad scroll for orbit/tilt.
    pending_trackpad_orbit: Vector2<f64>,

    sensitivity: f64,
}

impl UpdateState for CameraHandler {
    fn update_state(&mut self, MapContext { view_state, .. }: &mut MapContext, _dt: Duration) {
        self.turn(view_state);
    }
}

impl CameraHandler {
    /// Updates camera orbit while respecting `NavigationState` (`north_locked`, `2D` mode, and
    /// disengaging `Auto-Tilt` when the user manually tilts pitch).
    pub(crate) fn update_state_with_nav(
        &mut self,
        view_state: &mut ViewState,
        nav: &mut NavigationState,
    ) {
        if self.pending_trackpad_orbit.x.abs() > f64::EPSILON
            || self.pending_trackpad_orbit.y.abs() > f64::EPSILON
        {
            let delta = std::mem::replace(&mut self.pending_trackpad_orbit, Vector2::new(0.0, 0.0));
            if delta.x.abs() > 0.1 && !nav.north_locked {
                nav.on_manual_bearing();
                let current = view_state.camera().get_bearing();
                view_state
                    .camera_mut()
                    .set_bearing(current - Rad::from(Deg(delta.x * 0.35)));
            }
            if delta.y.abs() > 0.1 && nav.three_dimensional {
                nav.on_manual_tilt();
                let max_pitch = view_state.camera().max_pitch().0.to_degrees();
                let current = view_state.camera().get_pitch().0.to_degrees();
                let next = (current + delta.y * 0.35).clamp(0.0, max_pitch);
                super::navigation_state::set_pitch_keeping_center(view_state, Deg(next));
            }
        }
        if !self.is_active {
            return;
        }
        let (Some(window_position), Some(start_window_position)) =
            (self.window_position, self.start_window_position)
        else {
            return;
        };
        let drag_dx = (start_window_position.x - window_position.x).abs();
        let drag_dy = (start_window_position.y - window_position.y).abs();
        if drag_dx > 2.0 && !nav.north_locked {
            nav.on_manual_bearing();
        }
        if !self.is_middle && drag_dy > 4.0 {
            nav.on_manual_tilt();
        }
        self.turn_constrained(view_state, nav.north_locked, !nav.three_dimensional);
    }

    /// Turns the camera by the drag so far: the bearing by its horizontal travel, or by its
    /// length with the middle button, and the pitch by its vertical travel. A free-globe camera
    /// turns its pose about its target from the pose the drag started with.
    pub(crate) fn turn(&mut self, view_state: &mut ViewState) {
        self.turn_constrained(view_state, false, false);
    }

    fn turn_constrained(
        &mut self,
        view_state: &mut ViewState,
        lock_bearing: bool,
        lock_pitch: bool,
    ) {
        if !self.is_active {
            return;
        }
        let (Some(window_position), Some(start_window_position)) =
            (self.window_position, self.start_window_position)
        else {
            return;
        };
        let scale = |pixels: f64| -> Rad<f64> { (Deg(0.001 * self.sensitivity) * pixels).into() };
        let (mut bearing, mut pitch) = if self.is_middle {
            (
                scale(start_window_position.distance(window_position)),
                Rad(0.0),
            )
        } else {
            // Horizontal drag turns the bearing, as GL JS drag rotation does.
            (
                scale(start_window_position.x - window_position.x),
                scale(start_window_position.y - window_position.y),
            )
        };
        if lock_bearing {
            bearing = Rad(0.0);
        }
        if lock_pitch {
            pitch = Rad(0.0);
        }
        if view_state.navigation_mode() == NavigationMode::FreeGlobe {
            let Some(start) = self.start_pose.or_else(|| view_state.globe_pose()) else {
                return;
            };
            self.start_pose = Some(start);
            if let Err(error) = view_state.orbit_globe_pose(start, bearing, pitch) {
                tracing::warn!(%error, "free camera turn rejected");
            }
            return;
        }
        let camera = view_state.camera_mut();
        let previous = *self.start_delta_roll.get_or_insert(camera.get_bearing());
        if !lock_bearing {
            camera.set_bearing(previous + bearing);
        }
        if !self.is_middle && !lock_pitch {
            let max_pitch = camera.max_pitch().0;
            let previous = *self.start_delta_pitch.get_or_insert(camera.get_pitch());
            camera.set_pitch(Rad((previous + pitch).0.clamp(0.0, max_pitch)));
        }
    }

    pub(crate) fn new(sensitivity: f64) -> Self {
        Self {
            window_position: None,
            start_window_position: None,
            is_active: false,
            is_middle: false,
            start_delta_pitch: None,
            start_delta_roll: None,
            start_pose: None,
            pending_trackpad_orbit: Vector2::new(0.0, 0.0),
            sensitivity,
        }
    }

    /// Accumulates an `Alt` + two-finger trackpad scroll delta for manual bearing/pitch orbit.
    pub(crate) fn process_trackpad_orbit_scroll(&mut self, delta: Vector2<f64>) {
        self.pending_trackpad_orbit += delta;
    }

    pub(crate) fn is_active(&self) -> bool {
        self.is_active
    }

    pub(crate) fn begin_orbit(&mut self, is_middle: bool) {
        self.is_active = true;
        self.is_middle = is_middle;
        self.start_window_position = self.window_position;
        self.start_delta_pitch = None;
        self.start_delta_roll = None;
        self.start_pose = None;
    }

    pub(crate) fn end_orbit(&mut self) {
        self.is_active = false;
        self.is_middle = false;
        self.start_window_position = None;
        self.start_delta_pitch = None;
        self.start_delta_roll = None;
        self.start_pose = None;
    }

    pub(crate) fn process_window_position(
        &mut self,
        window_position: &Vector2<f64>,
        touch: bool,
    ) -> bool {
        if !self.is_active && !touch {
            self.start_window_position = Some(*window_position);
            self.window_position = Some(*window_position);
        } else {
            if self.start_window_position.is_none() {
                self.start_window_position = Some(*window_position);
            }
            self.window_position = Some(*window_position);
        }
        true
    }

    pub(crate) fn process_pointer_button(
        &mut self,
        button: PointerButton,
        state: ButtonState,
    ) -> bool {
        if state == ButtonState::Pressed {
            match button {
                PointerButton::Secondary => {
                    self.is_active = true;
                    self.start_window_position = self.window_position;
                }
                PointerButton::Middle => {
                    self.is_active = true;
                    self.is_middle = true;
                    self.start_window_position = self.window_position;
                }
                _ => return false,
            }
        } else {
            self.end_orbit();
        }
        true
    }
}

#[cfg(test)]
mod tests;
