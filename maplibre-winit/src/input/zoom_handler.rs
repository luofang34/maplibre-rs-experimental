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
use winit::{event::TouchPhase, keyboard::Key};

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
/// Pause threshold (`MKMapGestureController` `0.15s`) after which a trackpad pinch rebases its
/// cumulative scale and drops stale release velocity.
const PINCH_PAUSE_RESET: Duration = Duration::from_millis(150);
/// Quiet time after the last browser `ctrlKey + wheel` pinch event before triggering release inertia.
const WEB_PINCH_WHEEL_END_DELAY: Duration = Duration::from_millis(55);
/// Trackpad pinch-apart (zoom-in) magnification gain before Apple Maps' `1 + M` scale mapping.
const PINCH_ZOOM_IN_GAIN: f64 = 1.65;
/// Trackpad pinch-together (zoom-out) magnification gain before Apple Maps' non-linear power curve.
const PINCH_ZOOM_OUT_GAIN: f64 = 1.35;
/// Apple Maps (`UITransformEvent.scale`) zoom-out multiplier `K = 1.9` in `1 / (1 + K * (-M)^E)`.
const PINCH_ZOOM_OUT_K: f64 = 1.9;
/// Apple Maps (`UITransformEvent.scale`) zoom-out exponent `E = 1.1` in `1 / (1 + K * (-M)^E)`.
const PINCH_ZOOM_OUT_EXP: f64 = 1.1;
/// Exponential decay rate (`s^-1`) for post-pinch momentum (`VectorKit.dynamicValueAnimation`).
const PINCH_INERTIA_DECAY_RATE: f64 = 6.0;
/// Release velocity transfer factor into post-pinch inertia.
const PINCH_INERTIA_GAIN: f64 = 0.55;
/// Minimum pinch release speed (zoom levels / sec) required to trigger post-pinch inertia.
const PINCH_INERTIA_MIN_VELOCITY: f64 = 0.30;
/// Maximum post-pinch inertia speed (zoom levels / sec), matching `_MKZoomingGestureControlConfiguration`.
const PINCH_INERTIA_MAX_VELOCITY: f64 = 7.0;

/// Apple Maps (`UITransformEvent.scale`) asymmetric magnification-to-scale mapping:
/// linear `1 + M` when spreading fingers apart (`M >= 0`), and `1 / (1 + 1.9 * (-M)^1.1)`
/// when pinching fingers together (`M < 0`) so fingers do not collide before completing a zoom-out.
fn apple_pinch_scale(cumulative_mag: f64) -> f64 {
    if cumulative_mag >= 0.0 {
        1.0 + cumulative_mag
    } else {
        1.0 / (1.0 + PINCH_ZOOM_OUT_K * (-cumulative_mag).powf(PINCH_ZOOM_OUT_EXP))
    }
}

pub struct ZoomHandler {
    window_position: Option<Vector2<f64>>,
    zoom_delta: Option<Zoom>,
    /// Smoothly animated zoom remainder (used for `+`/`-` buttons, keys, and double-click).
    smooth_zoom_remaining: f64,
    /// Optional explicit anchor pixel for the active smooth zoom animation.
    smooth_zoom_anchor: Option<Vector2<f64>>,
    scroll: ScrollInput,
    /// Cumulative scaled magnification `M` within the current unidirectional pinch stroke.
    pinch_cumulative_mag: f64,
    /// Scale factor at the previous pinch update (`_lastScale` in `MKMapGestureController`).
    pinch_last_scale: f64,
    /// Filtered zoom velocity (zoom levels / sec) during an active pinch (`_UIPinchGestureRecognizerDriver`).
    pinch_velocity: f64,
    /// Timestamp of the last non-trivial pinch movement (`_lastPinchUpdateTimestamp`).
    last_pinch_move: Option<Instant>,
    /// Active post-pinch momentum velocity (zoom levels / sec) decaying via `PINCH_INERTIA_DECAY_RATE`.
    pinch_inertia_velocity: f64,
    /// Whether a browser `ctrlKey + wheel` trackpad pinch is currently open and awaiting release.
    web_pinch_wheel_active: bool,
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
        if self.web_pinch_wheel_active
            && self
                .last_pinch_move
                .is_some_and(|t| now.saturating_duration_since(t) >= WEB_PINCH_WHEEL_END_DELAY)
        {
            self.process_pinch_gesture_at(0.0, TouchPhase::Ended, now);
        }
        if let Some(zoom) = self.scroll.take_zoom(now) {
            self.smooth_zoom_anchor = None;
            self.pinch_inertia_velocity = 0.0;
            self.update_zoom(zoom);
        }
        if self.pinch_inertia_velocity.abs() > 0.05 {
            let dt_sec = dt.as_secs_f64().clamp(1.0 / 240.0, 0.1);
            let decay = (-PINCH_INERTIA_DECAY_RATE * dt_sec).exp();
            let step = self.pinch_inertia_velocity * (1.0 - decay) / PINCH_INERTIA_DECAY_RATE;
            self.pinch_inertia_velocity *= decay;
            if self.pinch_inertia_velocity.abs() <= 0.05 {
                self.pinch_inertia_velocity = 0.0;
            }
            self.smooth_zoom_anchor = None;
            self.update_zoom(step);
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
            pinch_cumulative_mag: 0.0,
            pinch_last_scale: 1.0,
            pinch_velocity: 0.0,
            last_pinch_move: None,
            pinch_inertia_velocity: 0.0,
            web_pinch_wheel_active: false,
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

    /// Stops any active post-pinch momentum coasting (e.g. when a pan or click starts).
    pub fn cancel_pinch_inertia(&mut self) {
        self.pinch_inertia_velocity = 0.0;
        self.web_pinch_wheel_active = false;
    }

    /// Queues a smooth animated zoom step (for HUD `+`/`-` buttons, keys, or double-click).
    pub fn add_smooth_zoom(&mut self, delta: f64, anchor: Option<Vector2<f64>>) {
        self.pinch_inertia_velocity = 0.0;
        self.smooth_zoom_remaining = (self.smooth_zoom_remaining + delta).clamp(-4.0, 4.0);
        self.smooth_zoom_anchor = anchor;
    }

    /// Shortest time until a pending scroll classification or gesture-end release needs a frame.
    pub(super) fn next_wait(&self, now: Instant) -> Option<Duration> {
        if self.zoom_delta.is_some()
            || self.smooth_zoom_remaining.abs() > 1e-4
            || self.pinch_inertia_velocity.abs() > 0.05
        {
            return Some(Duration::ZERO);
        }
        let scroll_wait = self.scroll.next_wait(now);
        let web_pinch_wait = self.web_pinch_wheel_active.then(|| {
            self.last_pinch_move.map_or(Duration::ZERO, |last| {
                WEB_PINCH_WHEEL_END_DELAY.saturating_sub(now.saturating_duration_since(last))
            })
        });
        let finish_wait = self
            .last_input
            .map(|last| GESTURE_END_DELAY.saturating_sub(now.saturating_duration_since(last)));
        [scroll_wait, web_pinch_wait, finish_wait]
            .into_iter()
            .flatten()
            .min()
    }

    /// Adds scroll input to the frame's, as GL JS's scroll zoom takes it.
    pub fn process_scroll(&mut self, delta: &winit::event::MouseScrollDelta, scale_factor: f64) {
        self.pinch_inertia_velocity = 0.0;
        self.scroll.add(delta, scale_factor, Instant::now());
    }

    /// Processes a trackpad pinch delta with phase (`Started`, `Moved`, `Ended`, `Cancelled`),
    /// applying Apple Maps' asymmetric scale curve (`UITransformEvent.scale`) and dynamic
    /// release momentum (`VectorKit.dynamicValueAnimation`).
    pub fn process_pinch_gesture(&mut self, magnification: f64, phase: TouchPhase) {
        self.process_pinch_gesture_at(magnification, phase, Instant::now());
    }

    pub(super) fn process_pinch_gesture_at(
        &mut self,
        magnification: f64,
        phase: TouchPhase,
        now: Instant,
    ) {
        match phase {
            TouchPhase::Started => {
                self.pinch_inertia_velocity = 0.0;
                self.web_pinch_wheel_active = false;
                self.pinch_cumulative_mag = 0.0;
                self.pinch_last_scale = 1.0;
                self.pinch_velocity = 0.0;
                self.last_pinch_move = None;
                if magnification.is_finite() && magnification.abs() > 1e-6 && magnification > -1.0 {
                    self.apply_pinch_step(magnification, now);
                }
            }
            TouchPhase::Moved => {
                self.pinch_inertia_velocity = 0.0;
                if !magnification.is_finite() || magnification <= -1.0 {
                    return;
                }
                self.apply_pinch_step(magnification, now);
            }
            TouchPhase::Ended => {
                let recently_moved = self
                    .last_pinch_move
                    .is_some_and(|t| now.saturating_duration_since(t) <= PINCH_PAUSE_RESET);
                if recently_moved && self.pinch_velocity.abs() >= PINCH_INERTIA_MIN_VELOCITY {
                    self.pinch_inertia_velocity = (self.pinch_velocity * PINCH_INERTIA_GAIN)
                        .clamp(-PINCH_INERTIA_MAX_VELOCITY, PINCH_INERTIA_MAX_VELOCITY);
                } else {
                    self.pinch_inertia_velocity = 0.0;
                }
                self.pinch_cumulative_mag = 0.0;
                self.pinch_last_scale = 1.0;
                self.pinch_velocity = 0.0;
                self.last_pinch_move = None;
                self.web_pinch_wheel_active = false;
            }
            TouchPhase::Cancelled => {
                self.pinch_inertia_velocity = 0.0;
                self.pinch_cumulative_mag = 0.0;
                self.pinch_last_scale = 1.0;
                self.pinch_velocity = 0.0;
                self.last_pinch_move = None;
                self.web_pinch_wheel_active = false;
            }
        }
    }

    fn apply_pinch_step(&mut self, magnification: f64, now: Instant) {
        if magnification.abs() <= 1e-9 {
            return;
        }
        let paused = self
            .last_pinch_move
            .is_some_and(|t| now.saturating_duration_since(t) > PINCH_PAUSE_RESET);
        let reversed = magnification * self.pinch_cumulative_mag < 0.0;
        if paused || reversed {
            self.pinch_cumulative_mag = 0.0;
            self.pinch_last_scale = 1.0;
            if paused {
                self.pinch_velocity = 0.0;
            }
        }

        let gain = if magnification >= 0.0 {
            PINCH_ZOOM_IN_GAIN
        } else {
            PINCH_ZOOM_OUT_GAIN
        };
        let scaled_mag = magnification * gain * self.scroll.sensitivity();
        self.pinch_cumulative_mag += scaled_mag;
        let new_scale = apple_pinch_scale(self.pinch_cumulative_mag);
        let scale_ratio = new_scale / self.pinch_last_scale;
        self.pinch_last_scale = new_scale;
        if scale_ratio <= 0.0 {
            return;
        }

        let dz = scale_ratio.log2();
        self.update_zoom(dz);

        if magnification.abs() > 1e-4 {
            let dt = self
                .last_pinch_move
                .map(|t| now.saturating_duration_since(t).as_secs_f64())
                .unwrap_or(1.0 / 60.0)
                .clamp(1.0 / 240.0, 0.15);
            let inst_vel = dz / dt;
            self.pinch_velocity = if self.last_pinch_move.is_some() && !paused {
                0.25 * self.pinch_velocity + 0.75 * inst_vel
            } else {
                inst_vel
            };
            self.last_pinch_move = Some(now);
        }
    }

    /// Processes a browser `ctrlKey + WheelEvent` trackpad pinch step, where browsers report
    /// `deltaY = -magnification * 100.0` without explicit gesture start/end phases.
    pub fn process_web_pinch_wheel(&mut self, magnification: f64) {
        let now = Instant::now();
        if !self.web_pinch_wheel_active
            || self
                .last_pinch_move
                .is_some_and(|t| now.saturating_duration_since(t) > PINCH_PAUSE_RESET)
        {
            self.process_pinch_gesture_at(0.0, TouchPhase::Started, now);
            self.web_pinch_wheel_active = true;
        }
        self.process_pinch_gesture_at(magnification, TouchPhase::Moved, now);
    }

    /// Adds a trackpad pinch delta (shorthand for `TouchPhase::Moved`).
    #[cfg(test)]
    pub fn process_pinch(&mut self, magnification: f64) {
        self.process_pinch_gesture(magnification, TouchPhase::Moved);
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
