//! Multi-touch gesture recognizer for iPhone, iPad, Android, and touch-enabled Web/Desktop.
//!
//! Coordinates 1-finger pan, 2-finger pinch-zoom + midpoint pan, 2-finger twist rotation
//! (including post-release angular momentum inertia), and 2-finger parallel vertical drag
//! for manual pitch tilt.

use std::time::Duration;

use cgmath::{Deg, InnerSpace, Rad, Vector2};
use instant::Instant;

use super::{
    navigation_state::{normalize_angle_signed, set_pitch_keeping_center, NavigationState},
    pan_handler::PanHandler,
    projection::pan_globe_by_pixels,
    zoom_handler::ZoomHandler,
    UpdateState,
};
use crate::{context::MapContext, render::view_state::ViewState, style::Style};

const TWIST_PAUSE_RESET: Duration = Duration::from_millis(150);
const ROTATE_INERTIA_DECAY_RATE: f64 = 6.5;
const ROTATE_INERTIA_GAIN: f64 = 0.55;
const ROTATE_INERTIA_MIN_VELOCITY: f64 = 0.35;
const ROTATE_INERTIA_MAX_VELOCITY: f64 = 8.0;

/// Active multi-touch state machine.
pub(crate) struct PinchHandler {
    /// Active touch points `(touch_id, logical_position)`.
    touches: Vec<(u64, Vector2<f64>)>,
    /// Initial 2-finger touch positions when the 2-finger gesture began.
    start_positions: Option<(Vector2<f64>, Vector2<f64>)>,
    /// Previous 2-finger midpoint in logical pixels.
    prev_midpoint: Option<Vector2<f64>>,
    /// Anchor midpoint retained for post-release twist inertia.
    inertia_midpoint: Option<Vector2<f64>>,
    /// Previous 2-finger separation distance in logical pixels.
    prev_distance: Option<f64>,
    /// Previous 2-finger angle in radians.
    prev_angle: Option<f64>,
    /// Accumulated twist angle from the start of the 2-finger gesture, used for deadzone gating.
    accumulated_twist_rad: f64,
    /// Whether the 2-finger twist has exceeded its activation threshold.
    pub(crate) twist_unlocked: bool,
    /// Filtered angular velocity (radians / sec) during an active 2-finger twist.
    twist_velocity: f64,
    /// Timestamp of the last non-trivial 2-finger twist step.
    last_twist_move: Option<Instant>,
    /// Active post-release rotation momentum velocity (radians / sec).
    pub(crate) rotate_inertia_velocity: f64,
    /// Pending midpoint pan delta accumulated since the last frame.
    pending_midpoint_pan: Vector2<f64>,
    /// Pending manual pitch delta in degrees from a 2-finger parallel vertical drag.
    pub(crate) pending_pitch_deg: f64,
    /// Pending bearing delta in radians from a 2-finger twist.
    pub(crate) pending_bearing_rad: f64,
}

impl Default for PinchHandler {
    fn default() -> Self {
        Self {
            touches: Vec::new(),
            start_positions: None,
            prev_midpoint: None,
            inertia_midpoint: None,
            prev_distance: None,
            prev_angle: None,
            accumulated_twist_rad: 0.0,
            twist_unlocked: false,
            twist_velocity: 0.0,
            last_twist_move: None,
            rotate_inertia_velocity: 0.0,
            pending_midpoint_pan: Vector2::new(0.0, 0.0),
            pending_pitch_deg: 0.0,
            pending_bearing_rad: 0.0,
        }
    }
}

impl UpdateState for PinchHandler {
    fn update_state(&mut self, map_context: &mut MapContext, dt: Duration) {
        let mut dummy_nav = NavigationState::default();
        self.update_parts_with_nav(
            &map_context.style,
            &mut map_context.view_state,
            dt,
            &mut dummy_nav,
        );
    }
}

impl PinchHandler {
    /// Number of currently active touch points.
    pub(crate) fn touch_count(&self) -> usize {
        self.touches.len()
    }

    /// Whether post-release angular rotation inertia is actively coasting.
    pub(crate) fn is_coasting(&self) -> bool {
        self.rotate_inertia_velocity.abs() > 0.02
    }

    /// Cancels any active post-release rotation inertia.
    pub(crate) fn cancel_rotate_inertia(&mut self) {
        self.rotate_inertia_velocity = 0.0;
    }

    /// Registers a touch start and transitions between 1-finger and 2-finger gestures.
    pub(crate) fn on_touch_start(
        &mut self,
        id: u64,
        position: Vector2<f64>,
        pan_handler: &mut PanHandler,
    ) {
        self.rotate_inertia_velocity = 0.0;
        if let Some(entry) = self.touches.iter_mut().find(|(tid, _)| *tid == id) {
            entry.1 = position;
        } else {
            self.touches.push((id, position));
        }

        match self.touches.len() {
            1 => {
                self.reset_two_finger_baseline();
                pan_handler.process_touch_start(&position);
            }
            2 => {
                // Stop 1-finger pan without inertia when a second finger touches down.
                pan_handler.cancel_pan();
                self.capture_two_finger_baseline();
            }
            _ => {
                pan_handler.cancel_pan();
            }
        }
    }

    /// Registers a touch movement and accumulates 1-finger pan or 2-finger pinch/rotate/tilt.
    pub(crate) fn on_touch_move_at(
        &mut self,
        id: u64,
        position: Vector2<f64>,
        pan_handler: &mut PanHandler,
        zoom_handler: &mut ZoomHandler,
        nav: &mut NavigationState,
        now: Instant,
    ) {
        let Some(entry) = self.touches.iter_mut().find(|(tid, _)| *tid == id) else {
            return;
        };
        entry.1 = position;

        if self.touches.len() == 1 {
            pan_handler.process_window_position(&position, true);
            zoom_handler.process_window_position(&position, true);
            return;
        }

        if self.touches.len() >= 2 {
            let p0 = self.touches[0].1;
            let p1 = self.touches[1].1;
            let midpoint = (p0 + p1) * 0.5;
            let diff = p1 - p0;
            let distance = diff.magnitude();
            let angle = diff.y.atan2(diff.x);

            if let (Some(prev_mid), Some(prev_dist), Some(prev_ang)) =
                (self.prev_midpoint, self.prev_distance, self.prev_angle)
            {
                let delta_mid = midpoint - prev_mid;
                let dist_change = (distance - prev_dist).abs();
                let d_angle = normalize_angle_signed(angle - prev_ang);
                let fingers_side_by_side = (p1.x - p0.x).abs() >= (p1.y - p0.y).abs() * 0.75;
                let (dy0, dy1) = self
                    .start_positions
                    .map_or((0.0, 0.0), |(s0, s1)| (p0.y - s0.y, p1.y - s1.y));
                let opposite_vertical = dy0 * dy1 < -4.0;

                if !nav.north_locked && distance > 24.0 {
                    self.accumulated_twist_rad += d_angle;
                    let both_moved = dy0.abs() > 2.0 && dy1.abs() > 2.0;
                    let mut applied_bearing_delta = 0.0;
                    if !self.twist_unlocked
                        && self.accumulated_twist_rad.abs() >= 2.5_f64.to_radians()
                        && (opposite_vertical
                            || !fingers_side_by_side
                            || (both_moved && (dy0 - dy1).abs() > (dy0 + dy1).abs() * 1.2))
                    {
                        self.twist_unlocked = true;
                        self.pending_pitch_deg = 0.0;
                        nav.on_manual_bearing();
                        applied_bearing_delta = -self.accumulated_twist_rad;
                        self.pending_bearing_rad += applied_bearing_delta;
                    } else if self.twist_unlocked {
                        nav.on_manual_bearing();
                        applied_bearing_delta = -d_angle;
                        self.pending_bearing_rad += applied_bearing_delta;
                    }
                    if self.twist_unlocked && d_angle.abs() > 1e-4 {
                        let paused = self
                            .last_twist_move
                            .is_some_and(|t| now.saturating_duration_since(t) > TWIST_PAUSE_RESET);
                        let dt = self
                            .last_twist_move
                            .map(|t| now.saturating_duration_since(t).as_secs_f64())
                            .unwrap_or(1.0 / 60.0)
                            .clamp(1.0 / 240.0, 0.15);
                        let inst_vel = applied_bearing_delta / dt;
                        self.twist_velocity = if self.last_twist_move.is_some() && !paused {
                            0.3 * self.twist_velocity + 0.7 * inst_vel
                        } else {
                            inst_vel
                        };
                        self.last_twist_move = Some(now);
                        self.inertia_midpoint = Some(midpoint);
                    }
                }

                // Two-finger parallel vertical drag tilts pitch (disengaging Auto-Tilt) only
                // when both fingers move in the same vertical direction rather than twisting.
                let parallel_vertical = !self.twist_unlocked
                    && !opposite_vertical
                    && fingers_side_by_side
                    && delta_mid.y.abs() > 0.5
                    && delta_mid.y.abs() > delta_mid.x.abs() * 1.35
                    && delta_mid.y.abs() > dist_change * 1.25;

                if parallel_vertical && nav.three_dimensional {
                    nav.on_manual_tilt();
                    self.pending_pitch_deg += -delta_mid.y * 0.42;
                } else {
                    // Midpoint pan + pinch zoom around the 2-finger midpoint.
                    self.pending_midpoint_pan += midpoint - prev_mid;
                    if distance > 12.0 && prev_dist > 12.0 {
                        let scale = distance / prev_dist;
                        let zoom_step = scale.log2();
                        if zoom_step.abs() > 1e-4 {
                            nav.on_manual_pan_or_zoom();
                            zoom_handler.process_window_position(&midpoint, true);
                            zoom_handler.update_zoom(zoom_step);
                        }
                    }
                }
            }

            self.prev_midpoint = Some(midpoint);
            self.prev_distance = Some(distance);
            self.prev_angle = Some(angle);
        }
    }

    /// Registers a touch end or cancellation, triggers rotation inertia if twisting, and cleanly
    /// rebases if 1 finger remains down.
    pub(crate) fn on_touch_end_at(&mut self, id: u64, pan_handler: &mut PanHandler, now: Instant) {
        let was_two_finger = self.touches.len() >= 2;
        self.touches.retain(|(tid, _)| *tid != id);
        if was_two_finger && self.touches.len() < 2 {
            let recently_twisted = self
                .last_twist_move
                .is_some_and(|t| now.saturating_duration_since(t) <= TWIST_PAUSE_RESET);
            if self.twist_unlocked
                && recently_twisted
                && self.twist_velocity.abs() >= ROTATE_INERTIA_MIN_VELOCITY
            {
                self.rotate_inertia_velocity = (self.twist_velocity * ROTATE_INERTIA_GAIN)
                    .clamp(-ROTATE_INERTIA_MAX_VELOCITY, ROTATE_INERTIA_MAX_VELOCITY);
            } else {
                self.rotate_inertia_velocity = 0.0;
            }
        }
        match self.touches.len() {
            0 => {
                self.reset_two_finger_baseline();
                pan_handler.process_touch_end_at(now);
            }
            1 => {
                self.reset_two_finger_baseline();
                let remaining = self.touches[0].1;
                pan_handler.rebase_touch(remaining);
            }
            _ => {
                self.capture_two_finger_baseline();
            }
        }
    }

    /// Applies accumulated 2-finger midpoint pan, twist rotation, post-twist rotation inertia,
    /// and manual tilt to the view.
    pub(crate) fn update_parts_with_nav(
        &mut self,
        style: &Style,
        view_state: &mut ViewState,
        dt: Duration,
        nav: &mut NavigationState,
    ) {
        if self.rotate_inertia_velocity.abs() > 0.02 {
            let dt_sec = dt.as_secs_f64().clamp(1.0 / 240.0, 0.1);
            let decay = (-ROTATE_INERTIA_DECAY_RATE * dt_sec).exp();
            let step = self.rotate_inertia_velocity * (1.0 - decay) / ROTATE_INERTIA_DECAY_RATE;
            self.rotate_inertia_velocity *= decay;
            if self.rotate_inertia_velocity.abs() <= 0.02 {
                self.rotate_inertia_velocity = 0.0;
            }
            self.pending_bearing_rad += step;
        }

        if self.pending_midpoint_pan.magnitude2() > f64::EPSILON {
            let delta = self.pending_midpoint_pan;
            self.pending_midpoint_pan = Vector2::new(0.0, 0.0);
            let cursor = self
                .prev_midpoint
                .unwrap_or_else(|| super::projection::center_pixel(view_state));
            let plane = view_state.center_elevation();
            if !pan_globe_by_pixels(style, view_state, cursor, delta) {
                super::projection::pan_plane_by_pixels(view_state, cursor, delta, plane);
            }
        }

        if self.pending_bearing_rad.abs() > f64::EPSILON {
            let d_bearing = self.pending_bearing_rad;
            self.pending_bearing_rad = 0.0;
            if !nav.north_locked {
                let pivot = self.prev_midpoint.or(self.inertia_midpoint);
                super::navigation_state::rotate_bearing_around_cursor(
                    style,
                    view_state,
                    pivot,
                    Rad(d_bearing),
                );
            }
        }

        if self.pending_pitch_deg.abs() > f64::EPSILON {
            let d_pitch = self.pending_pitch_deg;
            self.pending_pitch_deg = 0.0;
            if nav.three_dimensional {
                let max_pitch = view_state.camera().max_pitch().0.to_degrees();
                let current = view_state.camera().get_pitch().0.to_degrees();
                let next = (current + d_pitch).clamp(0.0, max_pitch);
                set_pitch_keeping_center(view_state, Deg(next));
            }
        }
    }

    fn capture_two_finger_baseline(&mut self) {
        if self.touches.len() >= 2 {
            let p0 = self.touches[0].1;
            let p1 = self.touches[1].1;
            let diff = p1 - p0;
            let mid = (p0 + p1) * 0.5;
            self.start_positions = Some((p0, p1));
            self.prev_midpoint = Some(mid);
            self.inertia_midpoint = Some(mid);
            self.prev_distance = Some(diff.magnitude());
            self.prev_angle = Some(diff.y.atan2(diff.x));
            self.accumulated_twist_rad = 0.0;
            self.twist_unlocked = false;
            self.twist_velocity = 0.0;
            self.last_twist_move = None;
        }
    }

    fn reset_two_finger_baseline(&mut self) {
        self.start_positions = None;
        self.prev_midpoint = None;
        self.prev_distance = None;
        self.prev_angle = None;
        self.accumulated_twist_rad = 0.0;
        self.twist_unlocked = false;
        self.twist_velocity = 0.0;
        self.last_twist_move = None;
    }
}

#[cfg(test)]
mod tests;
