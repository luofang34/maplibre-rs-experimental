use std::time::Duration;

use cgmath::{Deg, MetricSpace, Rad, Vector2};
use maplibre::{
    context::MapContext,
    render::view_state::{GlobePose, NavigationMode, ViewState},
};
use winit::event::{ElementState, MouseButton};

use super::UpdateState;

pub struct CameraHandler {
    window_position: Option<Vector2<f64>>,
    start_window_position: Option<Vector2<f64>>,
    is_active: bool,
    is_middle: bool,

    start_delta_pitch: Option<Rad<f64>>,
    start_delta_roll: Option<Rad<f64>>,
    /// The free-globe pose the drag started from.
    start_pose: Option<GlobePose>,

    sensitivity: f64,
}

impl UpdateState for CameraHandler {
    fn update_state(&mut self, MapContext { view_state, .. }: &mut MapContext, _dt: Duration) {
        self.turn(view_state);
    }
}

impl CameraHandler {
    /// Turns the camera by the drag so far: the bearing by its horizontal travel, or by its
    /// length with the middle button, and the pitch by its vertical travel. A free-globe camera
    /// turns its pose about its target from the pose the drag started with.
    fn turn(&mut self, view_state: &mut ViewState) {
        if !self.is_active {
            return;
        }
        let (Some(window_position), Some(start_window_position)) =
            (self.window_position, self.start_window_position)
        else {
            return;
        };
        let scale = |pixels: f64| -> Rad<f64> { (Deg(0.001 * self.sensitivity) * pixels).into() };
        let (bearing, pitch) = if self.is_middle {
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
        camera.set_bearing(previous + bearing);
        if !self.is_middle {
            let previous = *self.start_delta_pitch.get_or_insert(camera.get_pitch());
            camera.set_pitch(previous + pitch);
        }
    }
}

impl CameraHandler {
    pub fn new(sensitivity: f64) -> Self {
        Self {
            window_position: None,
            start_window_position: None,
            is_active: false,
            is_middle: false,
            start_delta_pitch: None,
            start_delta_roll: None,
            start_pose: None,
            sensitivity,
        }
    }

    pub fn process_window_position(&mut self, window_position: &Vector2<f64>, touch: bool) -> bool {
        if !self.is_active && !touch {
            self.start_window_position = Some(*window_position);
            self.window_position = Some(*window_position);
        } else {
            self.window_position = Some(*window_position);
        }

        true
    }

    pub fn process_mouse_key_press(&mut self, key: &MouseButton, state: &ElementState) -> bool {
        if *state == ElementState::Pressed {
            // currently panning or starting to pan
            match *key {
                MouseButton::Right => {
                    self.is_active = true;
                }
                MouseButton::Middle => {
                    self.is_active = true;
                    self.is_middle = true;
                }
                _ => return false,
            }
        } else {
            // finished panning
            self.is_active = false;
            self.is_middle = false;
            self.start_window_position = None;
            self.window_position = None;
            self.start_delta_pitch = None;
            self.start_delta_roll = None;
            self.start_pose = None;
        }
        true
    }
}

#[cfg(test)]
mod tests;
