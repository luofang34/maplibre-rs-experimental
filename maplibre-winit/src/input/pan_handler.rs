use std::time::Duration;

use cgmath::{Point2, Vector2};
use instant::Instant;
use maplibre::{
    context::MapContext,
    terrain::interaction::{begin_gesture, finish_gesture, resolve_gesture_anchor},
};
use winit::event::{ElementState, MouseButton};

use super::{
    inertia::PanInertia,
    projection::{center_pixel, pan_globe_by_pixels, pan_plane_by_pixels},
    UpdateState,
};

#[derive(Default)]
pub struct PanHandler {
    window_position: Option<Vector2<f64>>,
    last_window_position: Option<Vector2<f64>>,
    start_window_position: Option<Vector2<f64>>,
    is_panning: bool,
    inertia: PanInertia,
    /// Elevation of the plane the running gesture drags, captured when it starts.
    gesture_plane: Option<f64>,
}

impl UpdateState for PanHandler {
    fn update_state(
        &mut self,
        MapContext {
            style,
            view_state,
            world,
            ..
        }: &mut MapContext,
        _dt: Duration,
    ) {
        let now = Instant::now();
        if !self.is_panning {
            if let Some(delta) = self.inertia.step(now) {
                let center = center_pixel(view_state);
                let plane = self
                    .gesture_plane
                    .unwrap_or_else(|| view_state.center_elevation());
                if !pan_globe_by_pixels(style, view_state, center, delta) {
                    pan_plane_by_pixels(view_state, center, delta, plane);
                }
            } else if self.gesture_plane.take().is_some() {
                finish_gesture(style, view_state, world);
            }
            return;
        }
        let (Some(window_position), Some(start_window_position)) =
            (self.window_position, self.start_window_position)
        else {
            return;
        };
        let delta = window_position - self.last_window_position.unwrap_or(window_position);
        self.last_window_position = Some(window_position);
        self.inertia.record(now, delta);
        let plane = *self.gesture_plane.get_or_insert_with(|| {
            let anchor = resolve_gesture_anchor(
                style,
                view_state,
                world,
                Point2::new(start_window_position.x, start_window_position.y),
            );
            begin_gesture(view_state);
            anchor
                .elevation
                .unwrap_or_else(|| view_state.center_elevation())
        });
        if pan_globe_by_pixels(style, view_state, window_position, delta) {
            return;
        }
        pan_plane_by_pixels(view_state, window_position, delta, plane);
    }
}

impl PanHandler {
    pub fn process_touch_start(&mut self, window_position: &Vector2<f64>) -> bool {
        self.begin(Some(*window_position));
        true
    }

    pub fn process_touch_end(&mut self) -> bool {
        self.end();
        true
    }

    pub fn process_window_position(&mut self, window_position: &Vector2<f64>, touch: bool) -> bool {
        if !self.is_panning && !touch {
            self.start_window_position = Some(*window_position);
            self.last_window_position = Some(*window_position);
            self.window_position = Some(*window_position);
        } else {
            self.window_position = Some(*window_position);
        }
        true
    }

    pub fn process_mouse_key_press(&mut self, key: &MouseButton, state: &ElementState) -> bool {
        if *key != MouseButton::Left {
            return false;
        }
        if *state == ElementState::Pressed {
            self.begin(None);
        } else {
            self.end();
        }
        true
    }

    fn begin(&mut self, window_position: Option<Vector2<f64>>) {
        self.inertia.cancel();
        self.is_panning = true;
        if let Some(window_position) = window_position {
            self.start_window_position = Some(window_position);
            self.last_window_position = Some(window_position);
            self.window_position = Some(window_position);
        }
    }

    fn end(&mut self) {
        self.inertia.release(Instant::now());
        self.start_window_position = None;
        self.last_window_position = None;
        self.window_position = None;
        self.is_panning = false;
    }
}
