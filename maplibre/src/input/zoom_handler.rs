//! Scroll-wheel, trackpad pinch, smooth button/double-tap zoom, and post-pinch momentum handler.

use std::time::Duration;

use cgmath::{Deg, Point2, Vector2};
use instant::Instant;

use self::scroll::ScrollInput;
use super::{
    events::{ButtonState, MouseScrollDelta, NavKey, TouchPhase},
    navigation_state::{set_pitch_keeping_center, NavigationState},
    projection::{center_pixel, zoom_globe_around_pixel},
    UpdateState,
};
use crate::{
    context::MapContext,
    coords::Zoom,
    terrain::interaction::{
        begin_gesture, finish_gesture,
        globe::{terrain_anchor_at, zoom_globe_keeping_anchor, TerrainAnchor},
        resolve_gesture_anchor, zoom_mercator_around, GestureAnchor,
    },
};

mod scroll;

const MAX_ZOOM_STEP_PER_FRAME: f64 = 1.0;
const GESTURE_END_DELAY: Duration = Duration::from_millis(200);
const PINCH_PAUSE_RESET: Duration = Duration::from_millis(150);
const WEB_PINCH_WHEEL_END_DELAY: Duration = Duration::from_millis(55);
pub(crate) const PINCH_ZOOM_IN_GAIN: f64 = 1.65;
pub(crate) const PINCH_ZOOM_OUT_GAIN: f64 = 1.35;
pub(crate) const PINCH_ZOOM_OUT_K: f64 = 1.9;
pub(crate) const PINCH_ZOOM_OUT_EXP: f64 = 1.1;
const PINCH_INERTIA_DECAY_RATE: f64 = 6.0;
pub(crate) const PINCH_INERTIA_GAIN: f64 = 0.55;
const PINCH_INERTIA_MIN_VELOCITY: f64 = 0.30;
const PINCH_INERTIA_MAX_VELOCITY: f64 = 7.0;

/// Apple Maps (`UITransformEvent.scale`) asymmetric magnification-to-scale mapping.
pub(crate) fn apple_pinch_scale(cumulative_mag: f64) -> f64 {
    if cumulative_mag >= 0.0 {
        1.0 + cumulative_mag
    } else {
        1.0 / (1.0 + PINCH_ZOOM_OUT_K * (-cumulative_mag).powf(PINCH_ZOOM_OUT_EXP))
    }
}

pub(crate) struct ZoomHandler {
    window_position: Option<Vector2<f64>>,
    pub(crate) zoom_delta: Option<Zoom>,
    /// Smoothly animated zoom remainder (used for `+`/`-` buttons, keys, and double-click).
    smooth_zoom_remaining: f64,
    /// Optional explicit anchor pixel for the active smooth zoom animation.
    smooth_zoom_anchor: Option<Vector2<f64>>,
    scroll: ScrollInput,
    /// Cumulative scaled magnification `M` within the current unidirectional pinch stroke.
    pinch_cumulative_mag: f64,
    /// Scale factor at the previous pinch update.
    pinch_last_scale: f64,
    /// Filtered zoom velocity (zoom levels / sec) during an active pinch.
    pinch_velocity: f64,
    /// Timestamp of the last non-trivial pinch movement.
    last_pinch_move: Option<Instant>,
    /// Active post-pinch momentum velocity (zoom levels / sec) decaying via `PINCH_INERTIA_DECAY_RATE`.
    pub(crate) pinch_inertia_velocity: f64,
    /// Whether a browser `ctrlKey + wheel` trackpad pinch is currently open and awaiting release.
    web_pinch_wheel_active: bool,
    /// Elevation the running gesture anchors on, captured when it starts.
    gesture_elevation: Option<Option<f64>>,
    /// The terrain point a zoom on a globe orbiting the terrain keeps under the pointer.
    globe_anchor: Option<TerrainAnchor>,
    last_input: Option<Instant>,
}

impl UpdateState for ZoomHandler {
    fn update_state(&mut self, map_context: &mut MapContext, dt: Duration) {
        self.update_state_with_nav_at(map_context, dt, None, Instant::now());
    }
}

impl ZoomHandler {
    pub(crate) fn update_state_with_nav_at(
        &mut self,
        state: &mut MapContext,
        dt: Duration,
        nav: Option<&mut NavigationState>,
        now: Instant,
    ) {
        self.update_parts_with_nav_at(
            &state.style,
            &mut state.view_state,
            &mut state.world,
            dt,
            nav,
            now,
        );
    }

    pub(crate) fn update_parts_with_nav_at(
        &mut self,
        style: &crate::style::Style,
        view_state: &mut crate::render::view_state::ViewState,
        world: &mut crate::tcs::world::World,
        dt: Duration,
        nav: Option<&mut NavigationState>,
        now: Instant,
    ) {
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
            crate::terrain::elevation::ground_center(view_state, world);
        }
        let pointer = self
            .smooth_zoom_anchor
            .or(self.window_position)
            .unwrap_or_else(|| center_pixel(view_state));
        let current =
            resolve_gesture_anchor(style, view_state, world, Point2::new(pointer.x, pointer.y));
        let elevation = if auto_tilt_active {
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
        let next_zoom = view_state.clamp_zoom(Zoom::new(
            (view_state.zoom().value() + step).clamp(0.0, 22.0),
        ));
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
                    crate::terrain::elevation::ground_center(view_state, world);
                }
                let max_pitch = view_state.camera().max_pitch().0.to_degrees();
                let deg = nav_state.pitch_for_zoom(actual_zoom).clamp(0.0, max_pitch);
                set_pitch_keeping_center(view_state, Deg(deg));
                nav_state.cancel_pitch_animation();
            }
        }
        self.last_input = Some(now);
    }

    pub(crate) fn new(sensitivity: f64) -> Self {
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

    pub(crate) fn set_sensitivity(&mut self, sensitivity: f64) {
        self.scroll.set_sensitivity(sensitivity);
    }

    pub(crate) fn process_window_position(
        &mut self,
        window_position: &Vector2<f64>,
        _touch: bool,
    ) -> bool {
        self.window_position = Some(*window_position);
        true
    }

    pub(crate) fn update_zoom(&mut self, delta: f64) {
        self.zoom_delta = Some(self.zoom_delta.unwrap_or_default() + Zoom::new(delta));
    }

    pub(crate) fn cancel_pinch_inertia(&mut self) {
        self.pinch_inertia_velocity = 0.0;
        self.web_pinch_wheel_active = false;
    }

    pub(crate) fn add_smooth_zoom(&mut self, delta: f64, anchor: Option<Vector2<f64>>) {
        self.pinch_inertia_velocity = 0.0;
        self.smooth_zoom_remaining = (self.smooth_zoom_remaining + delta).clamp(-4.0, 4.0);
        self.smooth_zoom_anchor = anchor;
    }

    pub(crate) fn next_wait(&self, now: Instant) -> Option<Duration> {
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

    pub(crate) fn process_scroll_at(&mut self, delta: &MouseScrollDelta, now: Instant) {
        self.pinch_inertia_velocity = 0.0;
        self.scroll.add(delta, now);
    }

    pub(crate) fn process_pinch_gesture_at(
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

    pub(crate) fn process_web_pinch_wheel_at(&mut self, magnification: f64, now: Instant) {
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

    #[cfg(test)]
    pub(crate) fn process_pinch(&mut self, magnification: f64) {
        self.process_pinch_gesture_at(magnification, TouchPhase::Moved, Instant::now());
    }

    pub(crate) fn process_key(&mut self, key: NavKey, state: ButtonState) -> bool {
        if state != ButtonState::Pressed {
            return false;
        }
        match key {
            NavKey::ZoomIn => {
                self.add_smooth_zoom(0.65, None);
                true
            }
            NavKey::ZoomOut => {
                self.add_smooth_zoom(-0.65, None);
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests;
