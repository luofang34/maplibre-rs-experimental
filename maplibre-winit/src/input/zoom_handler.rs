use std::time::Duration;

use cgmath::{Deg, Point2, Vector2};
use instant::Instant;
use maplibre::{
    context::MapContext,
    coords::Zoom,
    terrain::interaction::{
        begin_gesture, finish_gesture,
        globe::{terrain_anchor_at, zoom_globe_keeping_anchor, TerrainAnchor},
        resolve_gesture_anchor, zoom_mercator_around, GestureAnchor,
    },
};
use winit::keyboard::Key;

use self::scroll::ScrollInput;
use super::{
    navigation_state::{set_pitch_keeping_center, NavigationState},
    projection::{center_pixel, zoom_globe_around_pixel},
    UpdateState,
};

mod scroll;

/// Largest zoom change applied per frame, as GL JS caps the scroll scale per frame at two.
const MAX_ZOOM_STEP_PER_FRAME: f64 = 1.0;
/// Quiet time after the last scroll input before the zoom gesture counts as finished.
const GESTURE_END_DELAY: Duration = Duration::from_millis(200);

pub struct ZoomHandler {
    window_position: Option<Vector2<f64>>,
    zoom_delta: Option<Zoom>,
    /// Smoothly animated zoom remainder (used for `+`/`-` buttons, keys, and double-click).
    smooth_zoom_remaining: f64,
    /// Optional explicit anchor pixel for the active smooth zoom animation.
    smooth_zoom_anchor: Option<Vector2<f64>>,
    scroll: ScrollInput,
    /// Elevation the running gesture anchors on, captured when it starts.
    gesture_elevation: Option<Option<f64>>,
    /// The terrain point a zoom on a globe orbiting the terrain keeps under the pointer,
    /// captured when the gesture starts.
    globe_anchor: Option<TerrainAnchor>,
    last_input: Option<Instant>,
}

impl UpdateState for ZoomHandler {
    fn update_state(&mut self, map_context: &mut MapContext, dt: Duration) {
        self.update_state_with_nav(map_context, dt, None);
    }
}

impl ZoomHandler {
    pub fn update_state_with_nav(
        &mut self,
        MapContext {
            style,
            view_state,
            world,
            ..
        }: &mut MapContext,
        dt: Duration,
        nav: Option<&mut NavigationState>,
    ) {
        let now = Instant::now();
        if let Some(zoom) = self.scroll.take_zoom(now) {
            self.smooth_zoom_anchor = None;
            self.update_zoom(zoom);
        }
        if self.smooth_zoom_remaining.abs() > 1e-4 {
            let alpha = (1.0 - (-dt.as_secs_f64() * 14.0).exp()).clamp(0.08, 1.0);
            let step = if self.smooth_zoom_remaining.abs() < 0.01 {
                let s = self.smooth_zoom_remaining;
                self.smooth_zoom_remaining = 0.0;
                self.smooth_zoom_anchor = None;
                s
            } else {
                let s = self.smooth_zoom_remaining * alpha;
                self.smooth_zoom_remaining -= s;
                s
            };
            self.update_zoom(step);
        }
        let Some(zoom_delta) = self.zoom_delta.take() else {
            let finished = self
                .last_input
                .is_some_and(|last| now.saturating_duration_since(last) >= GESTURE_END_DELAY);
            if finished {
                finish_gesture(style, view_state, world);
                self.gesture_elevation = None;
                self.globe_anchor = None;
                self.last_input = None;
            }
            return;
        };
        if (self.gesture_elevation.is_some() || self.globe_anchor.is_some())
            && self
                .last_input
                .is_some_and(|last| now.saturating_duration_since(last) >= GESTURE_END_DELAY)
        {
            finish_gesture(style, view_state, world);
            self.gesture_elevation = None;
            self.globe_anchor = None;
            self.last_input = None;
        }
        let step = zoom_delta
            .value()
            .clamp(-MAX_ZOOM_STEP_PER_FRAME, MAX_ZOOM_STEP_PER_FRAME);
        let remainder = zoom_delta.value() - step;
        if remainder.abs() > f64::EPSILON {
            self.zoom_delta = Some(Zoom::new(remainder));
        }
        let auto_tilt_active = nav
            .as_ref()
            .is_some_and(|n| n.three_dimensional && n.auto_tilt);
        if style.terrain.is_some()
            && (auto_tilt_active || view_state.center_elevation().abs() <= f64::EPSILON)
        {
            view_state.thaw_center_elevation();
            maplibre::terrain::elevation::ground_center(view_state, world);
        }
        let pointer = self
            .smooth_zoom_anchor
            .or(self.window_position)
            .unwrap_or_else(|| center_pixel(view_state));
        let current =
            resolve_gesture_anchor(style, view_state, world, Point2::new(pointer.x, pointer.y));
        let elevation = if auto_tilt_active {
            // Re-resolve the terrain anchor each frame while Auto-Tilt is simultaneously
            // tilting the camera so a stale elevation from many zoom levels ago never pulls
            // the orbit center underground.
            self.globe_anchor = terrain_anchor_at(style, view_state, world, current.pixel);
            current.elevation
        } else {
            match self.gesture_elevation {
                Some(elevation) => elevation,
                None => {
                    self.globe_anchor = terrain_anchor_at(style, view_state, world, current.pixel);
                    begin_gesture(view_state);
                    self.gesture_elevation = Some(current.elevation);
                    current.elevation
                }
            }
        };
        let anchor = GestureAnchor {
            pixel: current.pixel,
            elevation,
        };
        let next_zoom = Zoom::new((view_state.zoom().value() + step).clamp(0.0, 22.0));
        let mut zoomed_globe_anchor = false;
        if let Some(globe_anchor) = self.globe_anchor {
            if zoom_globe_keeping_anchor(view_state, globe_anchor, anchor.pixel, next_zoom.value())
            {
                zoomed_globe_anchor = true;
            }
        }
        if !zoomed_globe_anchor {
            let screen = Vector2::new(anchor.pixel.x, anchor.pixel.y);
            if !zoom_globe_around_pixel(style, view_state, screen, next_zoom) {
                zoom_mercator_around(view_state, anchor, next_zoom);
            }
        }
        if let Some(nav_state) = nav {
            let actual_zoom = view_state.zoom().value();
            nav_state.on_zoom_changed(actual_zoom);
            if nav_state.three_dimensional && nav_state.auto_tilt {
                if style.terrain.is_some() {
                    view_state.thaw_center_elevation();
                    maplibre::terrain::elevation::ground_center(view_state, world);
                }
                let max_pitch = view_state.camera().max_pitch().0.to_degrees();
                let deg = nav_state.pitch_for_zoom(actual_zoom).clamp(0.0, max_pitch);
                set_pitch_keeping_center(view_state, Deg(deg));
                nav_state.cancel_pitch_animation();
            }
        }
        self.last_input = Some(now);
    }
}

impl ZoomHandler {
    pub fn new(sensitivity: f64) -> Self {
        Self {
            window_position: None,
            zoom_delta: None,
            smooth_zoom_remaining: 0.0,
            smooth_zoom_anchor: None,
            scroll: ScrollInput::new(sensitivity),
            gesture_elevation: None,
            globe_anchor: None,
            last_input: None,
        }
    }

    /// Sets the scroll-wheel and trackpad zoom rate multiplier (`1.0` matches GL JS's default).
    pub fn set_sensitivity(&mut self, sensitivity: f64) {
        self.scroll.set_sensitivity(sensitivity);
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

    /// Queues a smooth animated zoom step (for HUD `+`/`-` buttons, keys, or double-click).
    pub fn add_smooth_zoom(&mut self, delta: f64, anchor: Option<Vector2<f64>>) {
        self.smooth_zoom_remaining = (self.smooth_zoom_remaining + delta).clamp(-4.0, 4.0);
        self.smooth_zoom_anchor = anchor;
    }

    /// Shortest time until a pending scroll classification or gesture-end release needs a frame.
    pub(super) fn next_wait(&self, now: Instant) -> Option<Duration> {
        if self.zoom_delta.is_some() || self.smooth_zoom_remaining.abs() > 1e-4 {
            return Some(Duration::ZERO);
        }
        let scroll_wait = self.scroll.next_wait(now);
        let finish_wait = self
            .last_input
            .map(|last| GESTURE_END_DELAY.saturating_sub(now.saturating_duration_since(last)));
        match (scroll_wait, finish_wait) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }

    /// Adds scroll input to the frame's, as GL JS's scroll zoom takes it.
    pub fn process_scroll(&mut self, delta: &winit::event::MouseScrollDelta, scale_factor: f64) {
        self.scroll.add(delta, scale_factor, Instant::now());
    }

    /// Adds a trackpad pinch, whose magnification scales the map by `1 + magnification` so
    /// that the map follows the fingers.
    pub fn process_pinch(&mut self, magnification: f64) {
        let scale = 1.0 + magnification;
        if scale > 0.0 {
            self.update_zoom(scale.log2());
        }
    }

    pub fn process_key_press(&mut self, key: &Key, state: winit::event::ElementState) -> bool {
        if state != winit::event::ElementState::Pressed {
            return false;
        }
        match key.as_ref() {
            Key::Character("i") | Key::Character("+") | Key::Character("=") => {
                self.add_smooth_zoom(0.65, None);
                true
            }
            Key::Character("k") | Key::Character("-") | Key::Character("_") => {
                self.add_smooth_zoom(-0.65, None);
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests;
