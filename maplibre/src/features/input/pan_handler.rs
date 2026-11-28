//! Single-pointer drag-pan, trackpad two-finger scroll pan, and inertial ease-out handler.

use std::time::Duration;

use cgmath::{Point2, Vector2};
use instant::Instant;

use super::{
    events::{ButtonState, PointerButton},
    inertia::PanInertia,
    projection::{center_pixel, pan_globe_by_pixels, pan_plane_by_pixels},
    UpdateState,
};
use crate::{
    context::MapContext,
    terrain::interaction::{
        begin_gesture, finish_gesture,
        globe::{place_anchor_at_pixel, terrain_anchor_at, TerrainAnchor},
        resolve_gesture_anchor,
    },
};

#[derive(Default)]
pub(crate) struct PanHandler {
    window_position: Option<Vector2<f64>>,
    last_window_position: Option<Vector2<f64>>,
    start_window_position: Option<Vector2<f64>>,
    is_panning: bool,
    inertia: PanInertia,
    /// Accumulated 2D pixel delta from a trackpad two-finger scroll pan (`PixelDelta`).
    pending_scroll_pan: Option<Vector2<f64>>,
    /// Elevation of the plane the running gesture drags, captured when it starts.
    gesture_plane: Option<f64>,
    /// The terrain point a drag on a globe orbiting the terrain keeps under the pointer, and
    /// the pixel it was last placed at.
    globe_anchor: Option<(TerrainAnchor, Vector2<f64>)>,
}

impl UpdateState for PanHandler {
    fn update_state(&mut self, state: &mut MapContext, dt: Duration) {
        self.update_state_at(state, dt, Instant::now());
    }
}

impl PanHandler {
    pub(crate) fn update_state_at(&mut self, state: &mut MapContext, _dt: Duration, now: Instant) {
        self.update_parts_at(&state.style, &mut state.view_state, &mut state.world, now);
    }

    pub(crate) fn update_parts_at(
        &mut self,
        style: &crate::style::Style,
        view_state: &mut crate::render::view_state::ViewState,
        world: &mut crate::tcs::world::World,
        now: Instant,
    ) {
        if let Some(delta) = self.pending_scroll_pan.take() {
            if delta.x.abs() > f64::EPSILON || delta.y.abs() > f64::EPSILON {
                let cursor = self
                    .window_position
                    .unwrap_or_else(|| center_pixel(view_state));
                let plane = view_state.center_elevation();
                if !pan_globe_by_pixels(style, view_state, cursor, delta) {
                    pan_plane_by_pixels(view_state, cursor, delta, plane);
                }
            }
        }
        if !self.is_panning {
            if let Some(delta) = self.inertia.step(now) {
                if let Some((anchor, pixel)) = &mut self.globe_anchor {
                    *pixel += delta;
                    if place_anchor_at_pixel(view_state, *anchor, Point2::new(pixel.x, pixel.y)) {
                        return;
                    }
                }
                let center = center_pixel(view_state);
                let plane = self
                    .gesture_plane
                    .unwrap_or_else(|| view_state.center_elevation());
                if !pan_globe_by_pixels(style, view_state, center, delta) {
                    pan_plane_by_pixels(view_state, center, delta, plane);
                }
            } else if self.gesture_plane.take().is_some() {
                self.globe_anchor = None;
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
        let start = Point2::new(start_window_position.x, start_window_position.y);
        if self.gesture_plane.is_some_and(|p| p.abs() <= f64::EPSILON)
            && view_state.center_elevation().abs() > f64::EPSILON
        {
            self.gesture_plane = Some(view_state.center_elevation());
        }
        let plane = match self.gesture_plane {
            Some(plane) => plane,
            None => {
                let anchor = resolve_gesture_anchor(style, view_state, world, start);
                self.globe_anchor = terrain_anchor_at(style, view_state, world, start)
                    .map(|anchor| (anchor, start_window_position));
                begin_gesture(view_state);
                let plane = anchor
                    .elevation
                    .unwrap_or_else(|| view_state.center_elevation());
                self.gesture_plane = Some(plane);
                plane
            }
        };
        if let Some((anchor, pixel)) = &mut self.globe_anchor {
            *pixel = window_position;
            let at = Point2::new(window_position.x, window_position.y);
            if place_anchor_at_pixel(view_state, *anchor, at) {
                return;
            }
        }
        if pan_globe_by_pixels(style, view_state, window_position, delta) {
            return;
        }
        pan_plane_by_pixels(view_state, window_position, delta, plane);
    }

    pub(crate) fn is_coasting(&self) -> bool {
        self.inertia.is_active()
    }

    pub(crate) fn process_touch_start(&mut self, window_position: &Vector2<f64>) -> bool {
        self.begin(Some(*window_position));
        true
    }

    pub(crate) fn process_touch_end_at(&mut self, now: Instant) -> bool {
        self.end_at(now);
        true
    }

    pub(crate) fn process_window_position(
        &mut self,
        window_position: &Vector2<f64>,
        touch: bool,
    ) -> bool {
        if !self.is_panning && !touch {
            self.start_window_position = Some(*window_position);
            self.last_window_position = Some(*window_position);
            self.window_position = Some(*window_position);
        } else {
            self.window_position = Some(*window_position);
        }
        true
    }

    pub(crate) fn process_pointer_button_at(
        &mut self,
        button: PointerButton,
        state: ButtonState,
        now: Instant,
    ) -> bool {
        if button != PointerButton::Primary {
            return false;
        }
        if state == ButtonState::Pressed {
            self.begin(None);
        } else {
            self.end_at(now);
        }
        true
    }

    pub(crate) fn is_panning(&self) -> bool {
        self.is_panning
    }

    /// Accumulates a 2D pixel pan delta from a trackpad two-finger scroll gesture.
    pub(crate) fn process_trackpad_scroll_pan(&mut self, delta_pixels: Vector2<f64>) {
        self.inertia.cancel();
        let current = self.pending_scroll_pan.unwrap_or(Vector2::new(0.0, 0.0));
        self.pending_scroll_pan = Some(current + delta_pixels);
    }

    pub(crate) fn begin_pan(&mut self, window_position: Option<Vector2<f64>>) {
        self.begin(window_position);
    }

    pub(crate) fn end_pan_at(&mut self, now: Instant) {
        self.end_at(now);
    }

    /// Stops an active 1-finger pan without starting inertia (e.g. when a second finger lands).
    pub(crate) fn cancel_pan(&mut self) {
        self.inertia.cancel();
        self.start_window_position = None;
        self.last_window_position = None;
        self.window_position = None;
        self.is_panning = false;
    }

    /// Rebases an active touch pan to a new finger position without releasing inertia, used when
    /// a multi-finger pinch transitions cleanly back to a single finger.
    pub(crate) fn rebase_touch(&mut self, window_position: Vector2<f64>) {
        self.inertia.cancel();
        self.is_panning = true;
        self.gesture_plane = None;
        self.globe_anchor = None;
        self.start_window_position = Some(window_position);
        self.last_window_position = Some(window_position);
        self.window_position = Some(window_position);
    }

    fn begin(&mut self, window_position: Option<Vector2<f64>>) {
        self.inertia.cancel();
        self.is_panning = true;
        self.gesture_plane = None;
        self.globe_anchor = None;
        if let Some(window_position) = window_position {
            self.start_window_position = Some(window_position);
            self.last_window_position = Some(window_position);
            self.window_position = Some(window_position);
        }
    }

    fn end_at(&mut self, now: Instant) {
        self.inertia.release(now);
        self.start_window_position = None;
        self.last_window_position = None;
        self.window_position = None;
        self.is_panning = false;
    }
}

#[cfg(test)]
mod tests;
