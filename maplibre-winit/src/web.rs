//! Canvas windows are created only on the active browser event loop.
use std::{cell::Cell, marker::PhantomData, sync::Arc};

use maplibre::window::{MapWindowConfig, WindowCreateError};
use winit::{platform::web::WindowAttributesExtWebSys, window::WindowAttributes};

use crate::{RawWinitWindow, WinitHostError, WinitMapWindow};

thread_local! {
    static SAFARI_GESTURE_ACTIVE: Cell<bool> = const { Cell::new(false) };
    static LAST_SAFARI_SCALE: Cell<f64> = const { Cell::new(1.0) };
    static LAST_SAFARI_ROTATION_DEG: Cell<f64> = const { Cell::new(0.0) };
    static PENDING_SAFARI_PINCH_DELTA: Cell<f64> = const { Cell::new(0.0) };
    static PENDING_SAFARI_ROTATION_DEG: Cell<f64> = const { Cell::new(0.0) };
}

/// Whether a Safari WebKit trackpad `GestureEvent` (`gesturestart`..`gestureend`) is currently active.
pub fn is_safari_gesture_active() -> bool {
    SAFARI_GESTURE_ACTIVE.with(Cell::get)
}

/// Takes any accumulated Safari WebKit trackpad `(pinch_delta, rotation_deg)` since the last frame.
/// `rotation_deg` is positive for counter-clockwise finger rotation.
pub fn take_safari_trackpad_gesture() -> (f64, f64) {
    let pinch = PENDING_SAFARI_PINCH_DELTA.with(|c| c.replace(0.0));
    let rotate = PENDING_SAFARI_ROTATION_DEG.with(|c| c.replace(0.0));
    (pinch, rotate)
}

/// Canvas selection and binding for a browser map.
#[derive(Clone)]
pub struct WinitMapWindowConfig<ET> {
    canvas_id: String,
    binding: crate::window::WindowBinding,
    phantom_et: PhantomData<ET>,
}
impl<ET: 'static + Clone> WinitMapWindowConfig<ET> {
    /// Selects an existing DOM canvas; the active event loop creates its window.
    pub fn new(canvas_id: String) -> Self {
        Self {
            canvas_id,
            binding: Default::default(),
            phantom_et: PhantomData,
        }
    }
    pub(crate) fn event_loop(&self) -> Result<crate::RawWinitEventLoop<ET>, WinitHostError> {
        winit::event_loop::EventLoop::<ET>::with_user_event()
            .build()
            .map_err(WinitHostError::EventLoop)
    }
    /// Creates the canvas window during the application's `resumed` callback.
    pub fn create_window(
        &self,
        active: &winit::event_loop::ActiveEventLoop,
    ) -> Result<WinitMapWindow<ET>, WinitHostError> {
        let canvas = get_canvas(&self.canvas_id)?;
        let window = active
            .create_window(WindowAttributes::default().with_canvas(Some(canvas.clone())))
            .map_err(WinitHostError::Window)?;
        let window = Arc::new(window);
        install_canvas_gesture_and_input_hooks(&canvas, &window);
        Ok(WinitMapWindow {
            window,
            display: active.owned_display_handle(),
            event: PhantomData,
        })
    }
    /// Binds map construction to a live canvas window without extending its lifetime.
    pub fn with_window(mut self, window: &WinitMapWindow<ET>) -> Self {
        self.binding = crate::window::WindowBinding::new(window);
        self
    }
}
impl<ET: 'static + Clone> MapWindowConfig for WinitMapWindowConfig<ET> {
    type MapWindow = WinitMapWindow<ET>;
    fn create(&self) -> Result<Self::MapWindow, WindowCreateError> {
        self.binding.create()
    }
}

#[wasm_bindgen::prelude::wasm_bindgen]
extern "C" {
    #[wasm_bindgen(extends = web_sys::Event)]
    type WebKitGestureEvent;

    #[wasm_bindgen(method, getter, structural)]
    fn scale(this: &WebKitGestureEvent) -> Option<f64>;

    #[wasm_bindgen(method, getter, structural)]
    fn rotation(this: &WebKitGestureEvent) -> Option<f64>;
}

fn read_gesture_scale(event: &web_sys::Event) -> f64 {
    use wasm_bindgen::JsCast;
    event
        .unchecked_ref::<WebKitGestureEvent>()
        .scale()
        .filter(|v| v.is_finite())
        .unwrap_or(1.0)
}

fn read_gesture_rotation(event: &web_sys::Event) -> f64 {
    use wasm_bindgen::JsCast;
    event
        .unchecked_ref::<WebKitGestureEvent>()
        .rotation()
        .filter(|v| v.is_finite())
        .unwrap_or(0.0)
}

/// Installs browser-specific input hooks on the canvas:
/// - Sets `touch-action: none` so multi-finger touch gestures are delivered to `winit`.
/// - Focuses the canvas before `winit`'s bubble-phase `wheel` listener runs so `ctrlKey + wheel`
///   pinch-zoom always reports `CONTROL` modifier state.
/// - Prevents default middle-click (`button == 1`) `mousedown`/`auxclick` behavior so pressing
///   the mouse wheel to rotate on Windows browsers does not trigger browser autoscroll.
/// - Listens to Safari's WebKit `gesturestart` / `gesturechange` / `gestureend` events so
///   two-finger trackpad rotation (and simultaneous pinch-zoom) works in Safari.
fn install_canvas_gesture_and_input_hooks(
    canvas: &web_sys::HtmlCanvasElement,
    window: &Arc<RawWinitWindow>,
) {
    use wasm_bindgen::{closure::Closure, JsCast};
    let _ = canvas.style().set_property("touch-action", "none");
    let _ = canvas.focus();

    let target = canvas.clone();
    let on_wheel =
        Closure::<dyn FnMut(web_sys::WheelEvent)>::new(move |_event: web_sys::WheelEvent| {
            let _ = target.focus();
        });
    let _ = canvas.add_event_listener_with_callback_and_bool(
        "wheel",
        on_wheel.as_ref().unchecked_ref(),
        true,
    );
    on_wheel.forget();

    let target_mouse = canvas.clone();
    let on_mouse =
        Closure::<dyn FnMut(web_sys::MouseEvent)>::new(move |event: web_sys::MouseEvent| {
            let _ = target_mouse.focus();
            if event.button() == 1 {
                event.prevent_default();
            }
        });
    let _ = canvas.add_event_listener_with_callback_and_bool(
        "mousedown",
        on_mouse.as_ref().unchecked_ref(),
        true,
    );
    let _ = canvas.add_event_listener_with_callback_and_bool(
        "auxclick",
        on_mouse.as_ref().unchecked_ref(),
        true,
    );
    on_mouse.forget();

    let on_gesture_start =
        Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
            event.prevent_default();
            let scale = read_gesture_scale(&event).max(1e-4);
            let rotation = read_gesture_rotation(&event);
            SAFARI_GESTURE_ACTIVE.with(|c| c.set(true));
            LAST_SAFARI_SCALE.with(|c| c.set(scale));
            LAST_SAFARI_ROTATION_DEG.with(|c| c.set(rotation));
        });
    let _ = canvas.add_event_listener_with_callback_and_bool(
        "gesturestart",
        on_gesture_start.as_ref().unchecked_ref(),
        false,
    );
    on_gesture_start.forget();

    let weak_window = Arc::downgrade(window);
    let on_gesture_change =
        Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
            event.prevent_default();
            let scale = read_gesture_scale(&event).max(1e-4);
            let rotation = read_gesture_rotation(&event);
            let prev_scale = LAST_SAFARI_SCALE.with(|c| c.replace(scale)).max(1e-4);
            let prev_rot = LAST_SAFARI_ROTATION_DEG.with(|c| c.replace(rotation));

            let zoom_delta = (scale / prev_scale).ln();
            let mut rot_delta = rotation - prev_rot;
            rot_delta = (rot_delta + 540.0).rem_euclid(360.0) - 180.0;

            let mut changed = false;
            if zoom_delta.abs() > 1e-5 {
                PENDING_SAFARI_PINCH_DELTA.with(|c| c.set(c.get() + zoom_delta));
                changed = true;
            }
            if rot_delta.abs() > 1e-4 {
                // WebKit GestureEvent.rotation is clockwise-positive; process_trackpad_rotate
                // expects counter-clockwise-positive (matching winit RotationGesture).
                PENDING_SAFARI_ROTATION_DEG.with(|c| c.set(c.get() - rot_delta));
                changed = true;
            }
            if changed {
                if let Some(w) = weak_window.upgrade() {
                    w.request_redraw();
                }
            }
        });
    let _ = canvas.add_event_listener_with_callback_and_bool(
        "gesturechange",
        on_gesture_change.as_ref().unchecked_ref(),
        false,
    );
    on_gesture_change.forget();

    let on_gesture_end = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
        event.prevent_default();
        SAFARI_GESTURE_ACTIVE.with(|c| c.set(false));
    });
    let _ = canvas.add_event_listener_with_callback_and_bool(
        "gestureend",
        on_gesture_end.as_ref().unchecked_ref(),
        false,
    );
    let _ = canvas.add_event_listener_with_callback_and_bool(
        "gesturecancel",
        on_gesture_end.as_ref().unchecked_ref(),
        false,
    );
    on_gesture_end.forget();
}

/// Resolves the configured canvas, preserving its ID in DOM selection errors.
pub fn get_canvas(element_id: &str) -> Result<web_sys::HtmlCanvasElement, WinitHostError> {
    use wasm_bindgen::JsCast;
    let document = web_sys::window()
        .and_then(|window| window.document())
        .ok_or(WinitHostError::DocumentUnavailable)?;
    document
        .get_element_by_id(element_id)
        .ok_or_else(|| WinitHostError::CanvasMissing {
            id: element_id.into(),
        })?
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .map_err(|_| WinitHostError::InvalidCanvas {
            id: element_id.into(),
        })
}
