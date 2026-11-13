//! Window-system-independent gesture recognition and camera input state machine.
//!
//! Hosts translate their native UI events (`winit`, UIKit, Android `MotionEvent`, Web DOM, or
//! custom embedded surfaces) into [`InputEvent`] values in logical window pixels and feed them
//! to [`InputController::handle_event`], then advance [`UpdateState::update_state`] (or
//! [`InputController::update_view`]) once per frame.

#![deny(missing_docs)]

use std::time::Duration;

use cgmath::{InnerSpace, Rad, Vector2};
use instant::Instant;

use crate::{
    context::MapContext,
    navigation::{NavigationState, PointerProfile},
};

mod camera_handler;
mod debug_handler;
mod events;
mod inertia;
mod pan_handler;
mod pinch_handler;
mod query_handler;
mod shift_handler;
mod update;
mod zoom_handler;

pub use events::{
    ButtonState, InputEvent, ModifiersState, MouseScrollDelta, NavKey, PointerButton, TouchPhase,
};
pub use inertia::PanInertia;

pub use crate::{navigation as navigation_state, navigation::projection};

#[cfg(test)]
mod tests;

use self::{
    camera_handler::CameraHandler, debug_handler::DebugHandler, pan_handler::PanHandler,
    pinch_handler::PinchHandler, query_handler::QueryHandler, shift_handler::ShiftHandler,
    zoom_handler::ZoomHandler,
};

/// Applies accumulated input and time-dependent motion to the map frame.
pub trait UpdateState {
    /// Advances input effects by the elapsed time since the preceding frame.
    fn update_state(&mut self, state: &mut MapContext, dt: Duration);
}

/// Platform-neutral gesture recognizer and camera controller.
///
/// Handles pointer drag pan, multi-touch 1-finger pan and 2-finger pinch-zoom / twist-rotate /
/// parallel-drag pitch, scroll-wheel and trackpad zoom/pan/orbit, double-click and double-tap
/// smooth zoom, keyboard navigation, HUD capsule interaction, and kinetic pan/zoom/rotate inertia.
pub struct InputController {
    pinch_handler: PinchHandler,
    pan_handler: PanHandler,
    pub(crate) zoom_handler: ZoomHandler,
    camera_handler: CameraHandler,
    shift_handler: ShiftHandler,
    query_handler: QueryHandler,
    debug_handler: DebugHandler,
    navigation_state: NavigationState,
    /// Touch ID currently interacting with the floating HUD capsule, if any.
    hud_touch_id: Option<u64>,
    /// Press position of the secondary pointer button in RTS mode before the drag threshold is met.
    rts_right_press_origin: Option<Vector2<f64>>,
    /// Whether the current secondary-button drag in RTS mode is actively panning the map.
    rts_right_panning: bool,
    /// Latest known viewport width in logical pixels.
    last_known_width: f64,
    /// Latest known viewport height in logical pixels.
    last_known_height: f64,
    /// Latest known continuous zoom level.
    last_known_zoom: f64,
    /// Latest known camera bearing.
    last_known_bearing: Rad<f64>,
    /// Exponentially smoothed camera pan velocity in world pixels per second for predictive prefetching.
    smoothed_pan_velocity: Vector2<f64>,
    /// Whether the last update moved the view or has ongoing inertia/animation requiring redraws.
    moved_view: bool,
}

impl InputController {
    /// Creates an input controller with movement `speed`, camera/keyboard `sensitivity`, and
    /// scroll/pinch `zoom_sensitivity` (`1.0` matches MapLibre GL JS's default zoom rates).
    pub fn new(speed: f64, sensitivity: f64, zoom_sensitivity: f64) -> Self {
        Self {
            pinch_handler: PinchHandler::default(),
            pan_handler: PanHandler::default(),
            zoom_handler: ZoomHandler::new(zoom_sensitivity),
            camera_handler: CameraHandler::new(sensitivity * 3.2),
            shift_handler: ShiftHandler::new(speed, sensitivity),
            query_handler: QueryHandler::new(),
            debug_handler: DebugHandler::default(),
            navigation_state: NavigationState::default(),
            hud_touch_id: None,
            rts_right_press_origin: None,
            rts_right_panning: false,
            last_known_width: 800.0,
            last_known_height: 800.0,
            last_known_zoom: 0.0,
            last_known_bearing: Rad(0.0),
            smoothed_pan_velocity: Vector2::new(0.0, 0.0),
            moved_view: false,
        }
    }

    /// Shared access to the underlying [`NavigationState`] (`three_dimensional`, `auto_tilt`,
    /// `north_locked`, `pointer_profile`, modifiers, and HUD flags).
    pub fn navigation_state(&self) -> &NavigationState {
        &self.navigation_state
    }

    /// Mutable access to the underlying [`NavigationState`].
    pub fn navigation_state_mut(&mut self) -> &mut NavigationState {
        &mut self.navigation_state
    }

    /// Whether the last frame's update moved the view or still carries active inertia/animation.
    pub fn moved_view(&self) -> bool {
        self.moved_view
    }

    /// Shortest time until a pending input timer (held wheel step, smooth animation, active
    /// pan/zoom/rotate inertia, held movement key, or gesture-end release) needs a frame.
    pub fn next_wait(&self, now: Instant) -> Option<Duration> {
        if self.navigation_state.is_animating()
            || self.shift_handler.is_active()
            || self.pan_handler.is_coasting()
            || self.pinch_handler.is_coasting()
        {
            return Some(Duration::ZERO);
        }
        self.zoom_handler.next_wait(now)
    }

    /// Processes a platform-neutral [`InputEvent`] stamped with [`Instant::now`].
    /// Returns `true` if the event was consumed by the map controller.
    pub fn handle_event(&mut self, event: InputEvent) -> bool {
        self.handle_event_at(event, Instant::now())
    }

    /// Processes a platform-neutral [`InputEvent`] at an explicit `now` timestamp.
    /// Returns `true` if the event was consumed by the map controller.
    pub fn handle_event_at(&mut self, event: InputEvent, now: Instant) -> bool {
        match event {
            InputEvent::ModifiersChanged(modifiers) => {
                self.navigation_state.shift_held = modifiers.shift;
                self.navigation_state.alt_held = modifiers.alt;
                self.navigation_state.ctrl_held = modifiers.ctrl;
                false
            }
            InputEvent::PointerMoved { position } => {
                let position = Vector2::new(position[0], position[1]);
                let consumed_by_hud = self.navigation_state.process_cursor_moved(
                    position,
                    self.last_known_width,
                    self.last_known_height,
                );

                if let Some(origin) = self.rts_right_press_origin {
                    if !self.rts_right_panning && (position - origin).magnitude() >= 4.0 {
                        self.rts_right_panning = true;
                        self.navigation_state.on_manual_pan_or_zoom();
                        self.pan_handler.begin_pan(Some(origin));
                    }
                }

                if !consumed_by_hud {
                    self.pan_handler.process_window_position(&position, false);
                    self.query_handler.process_window_position(&position, false);
                    self.zoom_handler.process_window_position(&position, false);
                    self.camera_handler
                        .process_window_position(&position, false);
                }
                true
            }
            InputEvent::Key { key, state } => {
                if state == ButtonState::Pressed {
                    match key {
                        NavKey::TogglePointerProfile => {
                            self.navigation_state.toggle_pointer_profile();
                            return true;
                        }
                        NavKey::Cycle3dMode => {
                            self.navigation_state.cycle_3d_mode(self.last_known_zoom);
                            return true;
                        }
                        NavKey::Compass => {
                            self.navigation_state
                                .trigger_compass(self.last_known_bearing);
                            return true;
                        }
                        NavKey::Home => {
                            self.navigation_state.trigger_home();
                            return true;
                        }
                        NavKey::ToggleHud => {
                            self.navigation_state.show_hud = !self.navigation_state.show_hud;
                            return true;
                        }
                        _ => {}
                    }
                }
                self.shift_handler.process_key(key, state)
                    || self.debug_handler.process_key(key, state)
                    || self.zoom_handler.process_key(key, state)
            }
            InputEvent::Touch {
                id,
                phase,
                position,
            } => {
                let position = Vector2::new(position[0], position[1]);
                match phase {
                    TouchPhase::Started => {
                        self.zoom_handler.cancel_pinch_inertia();
                        self.pinch_handler.cancel_rotate_inertia();
                        if self.pinch_handler.touch_count() == 0
                            && self.navigation_state.process_hud_press(
                                position,
                                self.last_known_width,
                                self.last_known_height,
                            )
                        {
                            self.hud_touch_id = Some(id);
                            return true;
                        }
                        if self.pinch_handler.touch_count() == 0
                            && self.navigation_state.check_double_click(now, position)
                        {
                            if let Some(delta) = self.navigation_state.take_pending_smooth_zoom() {
                                self.zoom_handler.add_smooth_zoom(delta, Some(position));
                            }
                            return true;
                        }
                        self.navigation_state.on_manual_pan_or_zoom();
                        self.pinch_handler
                            .on_touch_start(id, position, &mut self.pan_handler);
                        self.query_handler.process_touch_start();
                        true
                    }
                    TouchPhase::Moved => {
                        if self.hud_touch_id == Some(id) {
                            self.navigation_state.process_cursor_moved(
                                position,
                                self.last_known_width,
                                self.last_known_height,
                            );
                            return true;
                        }
                        self.pinch_handler.on_touch_move_at(
                            id,
                            position,
                            &mut self.pan_handler,
                            &mut self.zoom_handler,
                            &mut self.navigation_state,
                            now,
                        );
                        self.query_handler.process_window_position(&position, true);
                        self.camera_handler.process_window_position(&position, true);
                        true
                    }
                    TouchPhase::Ended | TouchPhase::Cancelled => {
                        if self.hud_touch_id == Some(id) {
                            self.hud_touch_id = None;
                            self.navigation_state
                                .process_hud_release(self.last_known_zoom, self.last_known_bearing);
                            if let Some(delta) = self.navigation_state.take_pending_smooth_zoom() {
                                let center = Vector2::new(
                                    self.last_known_width * 0.5,
                                    self.last_known_height * 0.5,
                                );
                                self.zoom_handler.add_smooth_zoom(delta, Some(center));
                            }
                            return true;
                        }
                        self.pinch_handler
                            .on_touch_end_at(id, &mut self.pan_handler, now);
                        self.query_handler.process_touch_end();
                        true
                    }
                }
            }
            InputEvent::PinchGesture { delta, phase } => {
                self.navigation_state.on_manual_pan_or_zoom();
                self.zoom_handler
                    .process_pinch_gesture_at(delta, phase, now);
                true
            }
            InputEvent::RotationGesture { delta_deg } => {
                self.navigation_state.process_trackpad_rotate(delta_deg);
                true
            }
            InputEvent::DoubleTapGesture => {
                self.navigation_state.on_manual_pan_or_zoom();
                let delta = if self.navigation_state.shift_held || self.navigation_state.alt_held {
                    -1.0
                } else {
                    1.0
                };
                self.zoom_handler
                    .add_smooth_zoom(delta, self.navigation_state.cursor_position);
                true
            }
            InputEvent::WebPinchWheel { magnification } => {
                if magnification.abs() > 1e-5 {
                    self.navigation_state.on_manual_pan_or_zoom();
                    self.zoom_handler
                        .process_web_pinch_wheel_at(magnification, now);
                }
                true
            }
            InputEvent::MouseWheel { delta } => {
                self.navigation_state.on_manual_pan_or_zoom();
                if let MouseScrollDelta::PixelDelta { x: dx, y: dy } = delta {
                    let is_discrete_wheel = dx == 0.0 && dy != 0.0 && {
                        let steps = dy / 4.000244140625;
                        (steps - steps.round()).abs() <= 1e-6 * steps.abs().max(1.0)
                            || (dy.abs() >= 40.0 && (dy % 10.0).abs() <= 1e-6)
                    };
                    if !is_discrete_wheel {
                        if self.navigation_state.ctrl_held {
                            // Browsers encode trackpad two-finger pinch as ctrlKey + wheel
                            // with deltaY = -magnification * 100.0 (negated into +dy).
                            self.zoom_handler.process_web_pinch_wheel_at(dy * 0.01, now);
                            return true;
                        }
                        self.zoom_handler.cancel_pinch_inertia();
                        self.pinch_handler.cancel_rotate_inertia();
                        if self.navigation_state.alt_held {
                            self.camera_handler
                                .process_trackpad_orbit_scroll(Vector2::new(dx, dy));
                        } else {
                            self.pan_handler
                                .process_trackpad_scroll_pan(Vector2::new(dx, dy));
                        }
                        return true;
                    }
                }
                self.zoom_handler.process_scroll_at(&delta, now);
                true
            }
            InputEvent::PointerButton { button, state } => {
                self.handle_pointer_button(button, state, now)
            }
        }
    }

    fn handle_pointer_button(
        &mut self,
        button: PointerButton,
        state: ButtonState,
        now: Instant,
    ) -> bool {
        let cursor = self.navigation_state.cursor_position;
        match (button, state) {
            (PointerButton::Primary, ButtonState::Pressed) => {
                self.zoom_handler.cancel_pinch_inertia();
                self.pinch_handler.cancel_rotate_inertia();
                if let Some(pos) = cursor {
                    if self.navigation_state.process_hud_press(
                        pos,
                        self.last_known_width,
                        self.last_known_height,
                    ) {
                        return true;
                    }
                    if self.navigation_state.check_double_click(now, pos) {
                        if let Some(delta) = self.navigation_state.take_pending_smooth_zoom() {
                            self.zoom_handler.add_smooth_zoom(delta, Some(pos));
                        }
                        return true;
                    }
                }
                if self.navigation_state.alt_held || self.navigation_state.ctrl_held {
                    self.camera_handler.begin_orbit(false);
                } else if self.navigation_state.pointer_profile == PointerProfile::Rts
                    || self.navigation_state.shift_held
                {
                    if let Some(pos) = cursor {
                        self.navigation_state.selection_drag = Some((pos, pos));
                    }
                } else {
                    self.navigation_state.on_manual_pan_or_zoom();
                    self.pan_handler
                        .process_pointer_button_at(button, state, now);
                }
                self.query_handler.process_pointer_button(button, state);
            }
            (PointerButton::Primary, ButtonState::Released) => {
                if self
                    .navigation_state
                    .process_hud_release(self.last_known_zoom, self.last_known_bearing)
                {
                    if let Some(delta) = self.navigation_state.take_pending_smooth_zoom() {
                        let center =
                            Vector2::new(self.last_known_width * 0.5, self.last_known_height * 0.5);
                        self.zoom_handler.add_smooth_zoom(delta, Some(center));
                    }
                    return true;
                }
                self.navigation_state.selection_drag = None;
                if self.camera_handler.is_active() {
                    self.camera_handler.end_orbit();
                }
                if self.pan_handler.is_panning() {
                    self.pan_handler.end_pan_at(now);
                }
                self.query_handler.process_pointer_button(button, state);
            }
            (PointerButton::Secondary, ButtonState::Pressed) => {
                self.pinch_handler.cancel_rotate_inertia();
                if self.navigation_state.pointer_profile == PointerProfile::Rts
                    && !self.navigation_state.alt_held
                {
                    self.rts_right_press_origin = cursor;
                    self.rts_right_panning = false;
                } else {
                    self.camera_handler.begin_orbit(false);
                }
            }
            (PointerButton::Secondary, ButtonState::Released) => {
                self.rts_right_press_origin = None;
                if self.rts_right_panning {
                    self.rts_right_panning = false;
                    self.pan_handler.end_pan_at(now);
                }
                if self.camera_handler.is_active() {
                    self.camera_handler.end_orbit();
                }
            }
            (PointerButton::Middle, ButtonState::Pressed) => {
                self.zoom_handler.cancel_pinch_inertia();
                self.pinch_handler.cancel_rotate_inertia();
                self.camera_handler.begin_orbit(false);
            }
            (PointerButton::Middle, ButtonState::Released) => {
                self.camera_handler.end_orbit();
            }
            _ => {
                self.pan_handler
                    .process_pointer_button_at(button, state, now);
                self.query_handler.process_pointer_button(button, state);
                self.camera_handler.process_pointer_button(button, state);
            }
        }
        true
    }
}
