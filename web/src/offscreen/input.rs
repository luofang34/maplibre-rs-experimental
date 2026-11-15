//! DOM input forwarding methods on [`OffscreenMapRunner`].

#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use maplibre::{
    input::{
        ButtonState, InputEvent, ModifiersState, MouseScrollDelta, NavKey, PointerButton,
        TouchPhase,
    },
    window::PhysicalSize,
};
use wasm_bindgen::prelude::*;

use super::{OffscreenMapRunner, RunnerInner};
use crate::controller::MapController;

#[wasm_bindgen]
impl OffscreenMapRunner {
    /// Returns the [`MapController`] bound to this offscreen map.
    pub fn controller(&self) -> MapController {
        self.controller.clone()
    }

    /// Updates physical pixel dimensions and CSS device pixel ratio.
    pub fn resize(&self, width: u32, height: u32, scale_factor: f64) {
        let Some(size) = PhysicalSize::new(width.max(1), height.max(1)) else {
            return;
        };
        let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
            scale_factor
        } else {
            1.0
        };
        if let Ok(mut inner) = self.inner.try_borrow_mut() {
            if inner.closed.get() {
                return;
            }
            inner.window_state.handle.0.set_width(size.width());
            inner.window_state.handle.0.set_height(size.height());
            inner.window_state.size.set(size);
            inner.window_state.scale_factor.set(scale);
            if let Ok(context) = inner.map.context_mut() {
                context.resize(size, scale);
            }
        }
        RunnerInner::schedule_raf(&self.inner);
    }

    /// Forwards logical CSS-pixel pointer motion.
    pub fn pointer_move(&self, x: f64, y: f64) {
        self.dispatch_input(InputEvent::PointerMoved { position: [x, y] });
    }

    /// Forwards logical CSS-pixel pointer press or release (`0` = primary, `1` = middle, `2` = secondary).
    pub fn pointer_button(&self, button: u16, pressed: bool, x: f64, y: f64) {
        let btn = match button {
            0 => PointerButton::Primary,
            1 => PointerButton::Middle,
            2 => PointerButton::Secondary,
            code => PointerButton::Other(code),
        };
        let state = if pressed {
            ButtonState::Pressed
        } else {
            ButtonState::Released
        };
        let changed = if let Ok(mut inner) = self.inner.try_borrow_mut() {
            let a = inner
                .input
                .handle_event(InputEvent::PointerMoved { position: [x, y] });
            let b = inner
                .input
                .handle_event(InputEvent::PointerButton { button: btn, state });
            a || b
        } else {
            false
        };
        if changed {
            RunnerInner::schedule_raf(&self.inner);
        }
    }

    /// Forwards wheel deltas (`mode`: `0` = CSS pixels, `1` = lines, `2` = pages).
    pub fn mouse_wheel(&self, dx: f64, dy: f64, mode: u32, ctrl: bool, shift: bool, alt: bool) {
        let delta = match mode {
            1 => MouseScrollDelta::LineDelta {
                x: -dx as f32,
                y: -dy as f32,
            },
            2 => MouseScrollDelta::LineDelta {
                x: (-dx * 3.0) as f32,
                y: (-dy * 3.0) as f32,
            },
            _ => MouseScrollDelta::PixelDelta { x: -dx, y: -dy },
        };
        let changed = if let Ok(mut inner) = self.inner.try_borrow_mut() {
            inner
                .input
                .handle_event(InputEvent::ModifiersChanged(ModifiersState {
                    shift,
                    alt,
                    ctrl,
                }));
            inner.input.handle_event(InputEvent::MouseWheel { delta })
        } else {
            false
        };
        if changed {
            RunnerInner::schedule_raf(&self.inner);
        }
    }

    /// Forwards touch points in logical CSS pixels (`phase`: `0` = start, `1` = move, `2` = end, `3` = cancel).
    pub fn touch(&self, id: u32, phase: u8, x: f64, y: f64) {
        let touch_phase = match phase {
            0 => TouchPhase::Started,
            1 => TouchPhase::Moved,
            2 => TouchPhase::Ended,
            _ => TouchPhase::Cancelled,
        };
        self.dispatch_input(InputEvent::Touch {
            id: u64::from(id),
            phase: touch_phase,
            position: [x, y],
        });
    }

    /// Forwards keyboard modifier flags.
    pub fn modifiers(&self, shift: bool, alt: bool, ctrl: bool) {
        self.dispatch_input(InputEvent::ModifiersChanged(ModifiersState {
            shift,
            alt,
            ctrl,
        }));
    }

    /// Forwards keyboard navigation presses (`key` matches `KeyboardEvent.key`).
    pub fn key(&self, key: &str, pressed: bool) {
        let state = if pressed {
            ButtonState::Pressed
        } else {
            ButtonState::Released
        };
        if let Some(nav_key) = translate_dom_key(key, state) {
            self.dispatch_input(InputEvent::Key {
                key: nav_key,
                state,
            });
        }
    }

    /// Stops the runner and releases GPU/worker resources.
    pub fn destroy(&self) {
        self.controller.remove();
        if let Ok(mut inner) = self.inner.try_borrow_mut() {
            inner.shutdown();
        }
    }

    fn dispatch_input(&self, event: InputEvent) {
        let consumed = self
            .inner
            .try_borrow_mut()
            .is_ok_and(|mut inner| !inner.closed.get() && inner.input.handle_event(event));
        if consumed {
            RunnerInner::schedule_raf(&self.inner);
        }
    }
}

fn translate_dom_key(key: &str, state: ButtonState) -> Option<NavKey> {
    match key {
        "Tab" => Some(NavKey::TogglePointerProfile),
        "t" | "T" => Some(NavKey::Cycle3dMode),
        "n" if state == ButtonState::Released => Some(NavKey::DebugInsetBottom),
        "n" | "N" => Some(NavKey::Compass),
        "r" | "R" | "Home" => Some(NavKey::Home),
        "h" | "H" => Some(NavKey::ToggleHud),
        "w" | "W" | "ArrowUp" => Some(NavKey::Up),
        "s" | "S" | "ArrowDown" => Some(NavKey::Down),
        "a" | "A" | "ArrowLeft" => Some(NavKey::Left),
        "d" | "D" | "ArrowRight" => Some(NavKey::Right),
        "q" | "Q" => Some(NavKey::OrbitCcw),
        "e" | "E" => Some(NavKey::OrbitCw),
        "i" | "+" | "=" => Some(NavKey::ZoomIn),
        "k" | "-" | "_" => Some(NavKey::ZoomOut),
        "v" => Some(NavKey::DebugInsetLeft),
        "b" => Some(NavKey::DebugInsetTop),
        "m" => Some(NavKey::DebugInsetRight),
        _ => None,
    }
}
