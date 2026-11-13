//! Thin `winit` adapter translating [`WindowEvent`]s into platform-neutral
//! [`maplibre::input::InputEvent`]s and delegating all gesture recognition to
//! [`maplibre::input::InputController`].

use std::time::Duration;

use maplibre::{
    context::MapContext,
    input::{
        ButtonState, InputController as CoreInputController, InputEvent, ModifiersState,
        MouseScrollDelta as CoreScrollDelta, NavKey, PointerButton, TouchPhase as CoreTouchPhase,
    },
};
pub use maplibre::{input::UpdateState, navigation as navigation_state, navigation::projection};
use winit::{
    event::{DeviceEvent, ElementState, KeyEvent, MouseButton, TouchPhase, WindowEvent},
    keyboard::{Key, NamedKey},
};

#[cfg(test)]
mod tests;

/// Accumulates `winit` window gestures and applies camera changes when the next frame updates.
pub struct InputController {
    inner: CoreInputController,
}

impl InputController {
    /// Sets movement speed, camera/keyboard sensitivity and scroll zoom sensitivity, a factor
    /// on GL JS's scroll zoom rates.
    pub fn new(speed: f64, sensitivity: f64, zoom_sensitivity: f64) -> Self {
        Self {
            inner: CoreInputController::new(speed, sensitivity, zoom_sensitivity),
        }
    }

    /// Shared reference to the underlying window-system-independent [`maplibre::input::InputController`].
    pub fn inner(&self) -> &CoreInputController {
        &self.inner
    }

    /// Mutable reference to the underlying window-system-independent [`maplibre::input::InputController`].
    pub fn inner_mut(&mut self) -> &mut CoreInputController {
        &mut self.inner
    }

    /// Whether the last frame's update moved the view: a drag's inertia, a held key or a zoom
    /// animation carries on into the next frame, which therefore has to be drawn.
    pub fn moved_view(&self) -> bool {
        self.inner.moved_view()
    }

    /// Shortest time until a pending input timer (such as a held wheel step, smooth animation,
    /// held movement key, or gesture-end release) needs a frame, even while the view itself is still.
    pub fn next_wait(&self, now: instant::Instant) -> Option<Duration> {
        self.inner.next_wait(now)
    }

    /// Leaves raw device events unconsumed; navigation is driven by window events.
    pub fn device_input(&mut self, _event: &DeviceEvent) -> bool {
        false
    }

    /// Translates the given [`winit::event::WindowEvent`] into a [`maplibre::input::InputEvent`]
    /// and forwards it to [`maplibre::input::InputController`].
    ///
    /// Returns `true` if the event was consumed and `false` otherwise.
    pub fn window_input(&mut self, event: &WindowEvent, scale_factor: f64) -> bool {
        let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
            scale_factor
        } else {
            1.0
        };

        match event {
            WindowEvent::ModifiersChanged(modifiers) => {
                let state = modifiers.state();
                self.inner
                    .handle_event(InputEvent::ModifiersChanged(ModifiersState {
                        shift: state.shift_key(),
                        alt: state.alt_key(),
                        ctrl: state.control_key() || state.super_key(),
                    }))
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.inner.handle_event(InputEvent::PointerMoved {
                    position: [position.x / scale, position.y / scale],
                })
            }
            WindowEvent::KeyboardInput {
                event: KeyEvent {
                    state, logical_key, ..
                },
                ..
            } => {
                let btn_state = translate_button_state(*state);
                let Some(nav_key) = translate_key(logical_key, btn_state) else {
                    return false;
                };
                self.inner.handle_event(InputEvent::Key {
                    key: nav_key,
                    state: btn_state,
                })
            }
            WindowEvent::Touch(touch) => self.inner.handle_event(InputEvent::Touch {
                id: touch.id,
                phase: translate_touch_phase(touch.phase),
                position: [touch.location.x / scale, touch.location.y / scale],
            }),
            WindowEvent::PinchGesture { delta, phase, .. } => {
                self.inner.handle_event(InputEvent::PinchGesture {
                    delta: *delta,
                    phase: translate_touch_phase(*phase),
                })
            }
            WindowEvent::RotationGesture { delta, .. } => {
                self.inner.handle_event(InputEvent::RotationGesture {
                    delta_deg: f64::from(*delta),
                })
            }
            WindowEvent::DoubleTapGesture { .. } => {
                self.inner.handle_event(InputEvent::DoubleTapGesture)
            }
            WindowEvent::MouseWheel { delta, .. } => {
                #[cfg(target_arch = "wasm32")]
                if let winit::event::MouseScrollDelta::PixelDelta(pos) = delta {
                    let dx = pos.x / scale;
                    let dy = pos.y / scale;
                    let is_discrete_wheel = dx == 0.0 && dy != 0.0 && {
                        let steps = dy / 4.000244140625;
                        (steps - steps.round()).abs() <= 1e-6 * steps.abs().max(1.0)
                            || (dy.abs() >= 40.0 && (dy % 10.0).abs() <= 1e-6)
                    };
                    if !is_discrete_wheel
                        && self.inner.navigation_state().ctrl_held
                        && crate::web::is_safari_gesture_active()
                    {
                        self.inner.navigation_state_mut().on_manual_pan_or_zoom();
                        return true;
                    }
                }
                let translated = match delta {
                    winit::event::MouseScrollDelta::LineDelta(x, y) => {
                        CoreScrollDelta::LineDelta { x: *x, y: *y }
                    }
                    winit::event::MouseScrollDelta::PixelDelta(pos) => {
                        CoreScrollDelta::PixelDelta {
                            x: pos.x / scale,
                            y: pos.y / scale,
                        }
                    }
                };
                self.inner
                    .handle_event(InputEvent::MouseWheel { delta: translated })
            }
            WindowEvent::MouseInput { button, state, .. } => {
                self.inner.handle_event(InputEvent::PointerButton {
                    button: translate_mouse_button(*button),
                    state: translate_button_state(*state),
                })
            }
            _ => false,
        }
    }
}

impl UpdateState for InputController {
    fn update_state(&mut self, map_context: &mut MapContext, dt: Duration) {
        #[cfg(target_arch = "wasm32")]
        {
            let (safari_pinch, safari_rotate_deg) = crate::web::take_safari_trackpad_gesture();
            if safari_pinch.abs() > 1e-5 {
                self.inner.handle_event(InputEvent::WebPinchWheel {
                    magnification: safari_pinch,
                });
            }
            if safari_rotate_deg.abs() > 1e-4 {
                self.inner.handle_event(InputEvent::RotationGesture {
                    delta_deg: safari_rotate_deg,
                });
            }
        }
        self.inner.update_state(map_context, dt);
    }
}

fn translate_button_state(state: ElementState) -> ButtonState {
    match state {
        ElementState::Pressed => ButtonState::Pressed,
        ElementState::Released => ButtonState::Released,
    }
}

fn translate_mouse_button(button: MouseButton) -> PointerButton {
    match button {
        MouseButton::Left => PointerButton::Primary,
        MouseButton::Right => PointerButton::Secondary,
        MouseButton::Middle => PointerButton::Middle,
        MouseButton::Back => PointerButton::Other(4),
        MouseButton::Forward => PointerButton::Other(5),
        MouseButton::Other(code) => PointerButton::Other(code),
    }
}

fn translate_touch_phase(phase: TouchPhase) -> CoreTouchPhase {
    match phase {
        TouchPhase::Started => CoreTouchPhase::Started,
        TouchPhase::Moved => CoreTouchPhase::Moved,
        TouchPhase::Ended => CoreTouchPhase::Ended,
        TouchPhase::Cancelled => CoreTouchPhase::Cancelled,
    }
}

fn translate_key(key: &Key, state: ButtonState) -> Option<NavKey> {
    match key.as_ref() {
        Key::Named(NamedKey::Tab) => Some(NavKey::TogglePointerProfile),
        Key::Character("t") | Key::Character("T") => Some(NavKey::Cycle3dMode),
        Key::Character("n") if state == ButtonState::Released => Some(NavKey::DebugInsetBottom),
        Key::Character("n") | Key::Character("N") => Some(NavKey::Compass),
        Key::Character("r") | Key::Character("R") | Key::Named(NamedKey::Home) => {
            Some(NavKey::Home)
        }
        Key::Character("h") | Key::Character("H") => Some(NavKey::ToggleHud),
        Key::Character("w") | Key::Character("W") | Key::Named(NamedKey::ArrowUp) => {
            Some(NavKey::Up)
        }
        Key::Character("s") | Key::Character("S") | Key::Named(NamedKey::ArrowDown) => {
            Some(NavKey::Down)
        }
        Key::Character("a") | Key::Character("A") | Key::Named(NamedKey::ArrowLeft) => {
            Some(NavKey::Left)
        }
        Key::Character("d") | Key::Character("D") | Key::Named(NamedKey::ArrowRight) => {
            Some(NavKey::Right)
        }
        Key::Character("q") | Key::Character("Q") => Some(NavKey::OrbitCcw),
        Key::Character("e") | Key::Character("E") => Some(NavKey::OrbitCw),
        Key::Character("i") | Key::Character("+") | Key::Character("=") => Some(NavKey::ZoomIn),
        Key::Character("k") | Key::Character("-") | Key::Character("_") => Some(NavKey::ZoomOut),
        Key::Character("v") => Some(NavKey::DebugInsetLeft),
        Key::Character("b") => Some(NavKey::DebugInsetTop),
        Key::Character("m") => Some(NavKey::DebugInsetRight),
        _ => None,
    }
}
