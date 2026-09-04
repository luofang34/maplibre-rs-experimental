use std::time::Duration;

use cgmath::{Point2, Vector2};
use instant::Instant;
use maplibre::{
    context::MapContext,
    coords::Zoom,
    terrain::interaction::{
        begin_gesture, finish_gesture, resolve_gesture_anchor, zoom_mercator_around, GestureAnchor,
    },
};
use winit::keyboard::Key;

use super::{
    projection::{center_pixel, zoom_globe_around_pixel},
    UpdateState,
};

/// Largest zoom change applied per frame, as GL JS caps the scroll scale per frame at two.
const MAX_ZOOM_STEP_PER_FRAME: f64 = 1.0;
/// Quiet time after the last scroll input before the zoom gesture counts as finished.
const GESTURE_END_DELAY: Duration = Duration::from_millis(200);

pub struct ZoomHandler {
    window_position: Option<Vector2<f64>>,
    zoom_delta: Option<Zoom>,
    sensitivity: f64,
    /// Elevation the running gesture anchors on, captured when it starts.
    gesture_elevation: Option<Option<f64>>,
    last_input: Option<Instant>,
}

impl UpdateState for ZoomHandler {
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
        let Some(zoom_delta) = self.zoom_delta.take() else {
            let finished = self
                .last_input
                .is_some_and(|last| now.duration_since(last) >= GESTURE_END_DELAY);
            if finished {
                finish_gesture(style, view_state, world);
                self.gesture_elevation = None;
                self.last_input = None;
            }
            return;
        };
        let step = zoom_delta
            .value()
            .clamp(-MAX_ZOOM_STEP_PER_FRAME, MAX_ZOOM_STEP_PER_FRAME);
        let remainder = zoom_delta.value() - step;
        if remainder.abs() > f64::EPSILON {
            self.zoom_delta = Some(Zoom::new(remainder));
        }
        let pointer = self
            .window_position
            .map_or_else(|| center_pixel(view_state), |position| position);
        let current =
            resolve_gesture_anchor(style, view_state, world, Point2::new(pointer.x, pointer.y));
        let elevation = *self.gesture_elevation.get_or_insert_with(|| {
            begin_gesture(view_state);
            current.elevation
        });
        let anchor = GestureAnchor {
            pixel: current.pixel,
            elevation,
        };
        let next_zoom = view_state.zoom() + Zoom::new(step);
        let screen = Vector2::new(anchor.pixel.x, anchor.pixel.y);
        if !zoom_globe_around_pixel(style, view_state, screen, next_zoom) {
            zoom_mercator_around(view_state, anchor, next_zoom);
        }
        self.last_input = Some(now);
    }
}

impl ZoomHandler {
    pub fn new(sensitivity: f64) -> Self {
        Self {
            window_position: None,
            zoom_delta: None,
            sensitivity,
            gesture_elevation: None,
            last_input: None,
        }
    }

    pub fn process_window_position(
        &mut self,
        window_position: &Vector2<f64>,
        _touch: bool,
    ) -> bool {
        self.window_position = Some(*window_position);
        true
    }

    pub fn update_zoom(&mut self, delta: f64) {
        self.zoom_delta = Some(self.zoom_delta.unwrap_or_default() + Zoom::new(delta));
    }

    pub fn process_scroll(&mut self, delta: &winit::event::MouseScrollDelta) {
        self.update_zoom(
            match delta {
                winit::event::MouseScrollDelta::LineDelta(_horizontal, vertical) => {
                    *vertical as f64
                }
                winit::event::MouseScrollDelta::PixelDelta(winit::dpi::PhysicalPosition {
                    y: scroll,
                    ..
                }) => *scroll / 100.0,
            } * self.sensitivity,
        );
    }

    pub fn process_key_press(&mut self, key: &Key, state: winit::event::ElementState) -> bool {
        let amount = if state == winit::event::ElementState::Pressed {
            0.1
        } else {
            0.0
        };

        match key.as_ref() {
            Key::Character("i") | Key::Character("+") => {
                self.update_zoom(amount);
                true
            }
            Key::Character("k") | Key::Character("-") => {
                self.update_zoom(-amount);
                true
            }
            _ => false,
        }
    }
}
