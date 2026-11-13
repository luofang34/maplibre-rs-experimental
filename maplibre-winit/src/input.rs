//! Handles the user input which is dispatched by the main event loop.

use std::time::Duration;

use cgmath::{InnerSpace, Rad, Vector2};
use instant::Instant;
use maplibre::context::MapContext;
use winit::{
    event::{DeviceEvent, ElementState, KeyEvent, MouseButton, TouchPhase, WindowEvent},
    keyboard::{Key, NamedKey},
};

use crate::input::{
    camera_handler::CameraHandler,
    debug_handler::DebugHandler,
    navigation_state::{NavigationState, PointerProfile},
    pan_handler::PanHandler,
    pinch_handler::PinchHandler,
    query_handler::QueryHandler,
    shift_handler::ShiftHandler,
    zoom_handler::ZoomHandler,
};

mod camera_handler;
mod debug_handler;
mod inertia;
pub use maplibre::navigation as navigation_state;
mod pan_handler;
mod pinch_handler;
pub use maplibre::navigation::projection;
mod query_handler;
mod shift_handler;
mod update;
mod zoom_handler;

/// Accumulates window gestures and applies camera changes when the next frame updates.
pub struct InputController {
    pinch_handler: PinchHandler,
    pan_handler: PanHandler,
    zoom_handler: ZoomHandler,
    camera_handler: CameraHandler,
    shift_handler: ShiftHandler,
    query_handler: QueryHandler,
    debug_handler: DebugHandler,
    navigation_state: NavigationState,
    /// Touch ID currently interacting with the floating HUD capsule, if any.
    hud_touch_id: Option<u64>,
    /// Press position of the right mouse button in RTS mode before the drag threshold is met.
    rts_right_press_origin: Option<Vector2<f64>>,
    /// Whether the current right-mouse drag in RTS mode is actively panning the map.
    rts_right_panning: bool,
    /// Latest known viewport width in logical pixels.
    last_known_width: f64,
    /// Latest known viewport height in logical pixels.
    last_known_height: f64,
    /// Latest known continuous zoom level.
    last_known_zoom: f64,
    /// Latest known camera bearing.
    last_known_bearing: Rad<f64>,
    /// Exponentially smoothed camera pan velocity in world pixels per second for predictive tile prefetching.
    smoothed_pan_velocity: Vector2<f64>,
    /// Whether the last update moved the view or updated the HUD, so the next frame may move it further.
    moved_view: bool,
}

impl InputController {
    /// Sets movement speed, camera/keyboard sensitivity and scroll zoom sensitivity, a factor
    /// on GL JS's scroll zoom rates.
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

    /// Whether the last frame's update moved the view: a drag's inertia, a held key or a zoom
    /// animation carries on into the next frame, which therefore has to be drawn.
    pub fn moved_view(&self) -> bool {
        self.moved_view
    }

    /// Shortest time until a pending input timer (such as a held wheel step, smooth animation,
    /// held movement key, or gesture-end release) needs a frame, even while the view itself is still.
    pub fn next_wait(&self, now: instant::Instant) -> Option<Duration> {
        if self.navigation_state.is_animating() || self.shift_handler.is_active() {
            return Some(Duration::ZERO);
        }
        self.zoom_handler.next_wait(now)
    }

    /// Leaves raw device events unconsumed; navigation is driven by window events.
    pub fn device_input(&mut self, _event: &DeviceEvent) -> bool {
        false
    }

    /// Process the given winit `[winit::event::WindowEvent]`.
    /// Returns true if the event has been processed and false otherwise.
    pub fn window_input(&mut self, event: &WindowEvent, scale_factor: f64) -> bool {
        match event {
            WindowEvent::ModifiersChanged(modifiers) => {
                let state = modifiers.state();
                self.navigation_state.shift_held = state.shift_key();
                self.navigation_state.alt_held = state.alt_key();
                self.navigation_state.ctrl_held = state.control_key() || state.super_key();
                false
            }
            WindowEvent::CursorMoved { position, .. } => {
                let position: (f64, f64) = position.to_owned().into();
                let position = Vector2::from(position) / scale_factor;

                let consumed_by_hud = self.navigation_state.process_cursor_moved(
                    position,
                    self.last_known_width,
                    self.last_known_height,
                );

                // In RTS mode, a right-button drag starts panning once it moves past 4px.
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
            WindowEvent::KeyboardInput {
                event: KeyEvent {
                    state, logical_key, ..
                },
                ..
            } => {
                if *state == ElementState::Pressed {
                    match logical_key.as_ref() {
                        Key::Named(NamedKey::Tab) => {
                            self.navigation_state.toggle_pointer_profile();
                            return true;
                        }
                        Key::Character("t") | Key::Character("T") => {
                            self.navigation_state.cycle_3d_mode(self.last_known_zoom);
                            return true;
                        }
                        Key::Character("n") | Key::Character("N") => {
                            self.navigation_state
                                .trigger_compass(self.last_known_bearing);
                            return true;
                        }
                        Key::Character("r") | Key::Character("R") | Key::Named(NamedKey::Home) => {
                            self.navigation_state.trigger_home();
                            return true;
                        }
                        Key::Character("h") | Key::Character("H") => {
                            self.navigation_state.show_hud = !self.navigation_state.show_hud;
                            return true;
                        }
                        _ => {}
                    }
                }
                self.shift_handler.process_key_press(logical_key, *state)
                    || self.debug_handler.process_key_press(logical_key, *state)
                    || self.zoom_handler.process_key_press(logical_key, *state)
            }
            WindowEvent::Touch(touch) => {
                let raw: (f64, f64) = touch.location.to_owned().into();
                let position = Vector2::from(raw) / scale_factor;
                match touch.phase {
                    TouchPhase::Started => {
                        if self.pinch_handler.touch_count() == 0
                            && self.navigation_state.process_hud_press(
                                position,
                                self.last_known_width,
                                self.last_known_height,
                            )
                        {
                            self.hud_touch_id = Some(touch.id);
                            return true;
                        }
                        if self.pinch_handler.touch_count() == 0
                            && self
                                .navigation_state
                                .check_double_click(Instant::now(), position)
                        {
                            if let Some(delta) = self.navigation_state.take_pending_smooth_zoom() {
                                self.zoom_handler.add_smooth_zoom(delta, Some(position));
                            }
                            return true;
                        }
                        self.navigation_state.on_manual_pan_or_zoom();
                        self.pinch_handler.on_touch_start(
                            touch.id,
                            position,
                            &mut self.pan_handler,
                        );
                        self.query_handler.process_touch_start();
                        true
                    }
                    TouchPhase::Moved => {
                        if self.hud_touch_id == Some(touch.id) {
                            self.navigation_state.process_cursor_moved(
                                position,
                                self.last_known_width,
                                self.last_known_height,
                            );
                            return true;
                        }
                        self.pinch_handler.on_touch_move(
                            touch.id,
                            position,
                            &mut self.pan_handler,
                            &mut self.zoom_handler,
                            &mut self.navigation_state,
                        );
                        self.query_handler.process_window_position(&position, true);
                        self.camera_handler.process_window_position(&position, true);
                        true
                    }
                    TouchPhase::Ended | TouchPhase::Cancelled => {
                        if self.hud_touch_id == Some(touch.id) {
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
                            .on_touch_end(touch.id, &mut self.pan_handler);
                        self.query_handler.process_touch_end();
                        true
                    }
                }
            }
            WindowEvent::PinchGesture { delta, phase, .. } => {
                self.navigation_state.on_manual_pan_or_zoom();
                self.zoom_handler.process_pinch_gesture(*delta, *phase);
                true
            }
            WindowEvent::RotationGesture { delta, .. } => {
                self.navigation_state
                    .process_trackpad_rotate(f64::from(*delta));
                true
            }
            WindowEvent::DoubleTapGesture { .. } => {
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
            WindowEvent::MouseWheel { delta, .. } => {
                self.navigation_state.on_manual_pan_or_zoom();
                if let winit::event::MouseScrollDelta::PixelDelta(pos) = delta {
                    let dx = pos.x / scale_factor;
                    let dy = pos.y / scale_factor;
                    let is_discrete_wheel = dx == 0.0 && dy != 0.0 && {
                        let steps = dy / 4.000244140625;
                        (steps - steps.round()).abs() <= 1e-6 * steps.abs().max(1.0)
                            || (dy.abs() >= 40.0 && (dy % 10.0).abs() <= 1e-6)
                    };
                    if !is_discrete_wheel {
                        if self.navigation_state.ctrl_held {
                            #[cfg(target_arch = "wasm32")]
                            if crate::web::is_safari_gesture_active() {
                                return true;
                            }
                            // Browsers encode trackpad two-finger pinch as ctrlKey + wheel
                            // with deltaY = -magnification * 100.0 (winit negates deltaY into pos.y).
                            self.zoom_handler.process_web_pinch_wheel(dy * 0.01);
                            return true;
                        }
                        self.zoom_handler.cancel_pinch_inertia();
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
                self.shift_handler.process_scroll(delta);
                self.zoom_handler.process_scroll(delta, scale_factor);
                true
            }
            WindowEvent::MouseInput { button, state, .. } => {
                let cursor = self.navigation_state.cursor_position;
                match (*button, *state) {
                    (MouseButton::Left, ElementState::Pressed) => {
                        self.zoom_handler.cancel_pinch_inertia();
                        if let Some(pos) = cursor {
                            if self.navigation_state.process_hud_press(
                                pos,
                                self.last_known_width,
                                self.last_known_height,
                            ) {
                                return true;
                            }
                            if self
                                .navigation_state
                                .check_double_click(Instant::now(), pos)
                            {
                                if let Some(delta) =
                                    self.navigation_state.take_pending_smooth_zoom()
                                {
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
                            self.pan_handler.process_mouse_key_press(button, state);
                        }
                        self.query_handler.process_mouse_key_press(button, state);
                    }
                    (MouseButton::Left, ElementState::Released) => {
                        if self
                            .navigation_state
                            .process_hud_release(self.last_known_zoom, self.last_known_bearing)
                        {
                            if let Some(delta) = self.navigation_state.take_pending_smooth_zoom() {
                                let center = Vector2::new(
                                    self.last_known_width * 0.5,
                                    self.last_known_height * 0.5,
                                );
                                self.zoom_handler.add_smooth_zoom(delta, Some(center));
                            }
                            return true;
                        }
                        self.navigation_state.selection_drag = None;
                        if self.camera_handler.is_active() {
                            self.camera_handler.end_orbit();
                        }
                        if self.pan_handler.is_panning() {
                            self.pan_handler.end_pan();
                        }
                        self.query_handler.process_mouse_key_press(button, state);
                    }
                    (MouseButton::Right, ElementState::Pressed) => {
                        if self.navigation_state.pointer_profile == PointerProfile::Rts
                            && !self.navigation_state.alt_held
                        {
                            self.rts_right_press_origin = cursor;
                            self.rts_right_panning = false;
                        } else {
                            self.camera_handler.begin_orbit(false);
                        }
                    }
                    (MouseButton::Right, ElementState::Released) => {
                        self.rts_right_press_origin = None;
                        if self.rts_right_panning {
                            self.rts_right_panning = false;
                            self.pan_handler.end_pan();
                        }
                        if self.camera_handler.is_active() {
                            self.camera_handler.end_orbit();
                        }
                    }
                    (MouseButton::Middle, ElementState::Pressed) => {
                        self.zoom_handler.cancel_pinch_inertia();
                        // Pressing the mouse wheel and moving rotates bearing (horizontal)
                        // and pitch (vertical) across both Map and RTS profiles.
                        self.camera_handler.begin_orbit(false);
                    }
                    (MouseButton::Middle, ElementState::Released) => {
                        self.camera_handler.end_orbit();
                    }
                    _ => {
                        self.pan_handler.process_mouse_key_press(button, state);
                        self.query_handler.process_mouse_key_press(button, state);
                        self.camera_handler.process_mouse_key_press(button, state);
                    }
                }
                true
            }
            _ => false,
        }
    }
}

/// Applies accumulated input and time-dependent motion to the map frame.
pub trait UpdateState {
    /// Advances input effects by the elapsed time since the preceding frame.
    fn update_state(&mut self, state: &mut MapContext, dt: Duration);
}
