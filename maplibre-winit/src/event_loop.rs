//! Resumed window creation and event dispatch for native and browser hosts.
#[cfg(target_arch = "wasm32")]
use maplibre::map::handle::MapHandle;
use maplibre::{
    environment::Environment,
    event_loop::{EventLoopProxy, SendEventError},
    map::{handle::WakeHost, Map},
};

use crate::{WinitApplicationError, WinitHostError, WinitMapWindowConfig};

mod dispatch;
mod lifecycle;
pub use lifecycle::Application as WinitApplication;
#[cfg(target_arch = "wasm32")]
mod completion;
mod initialization;

#[cfg(target_arch = "wasm32")]
pub(crate) type ActiveWindowSlot =
    std::rc::Rc<std::cell::RefCell<Option<std::sync::Weak<crate::RawWinitWindow>>>>;

/// Platform event loop for application events.
pub type RawWinitEventLoop<ET> = winit::event_loop::EventLoop<ET>;
/// Platform user-event sender.
pub type RawEventLoopProxy<ET> = winit::event_loop::EventLoopProxy<ET>;

/// Owns event dispatch independently of any window or presentation surface.
pub struct WinitEventLoop<ET: 'static> {
    pub(crate) event_loop: RawWinitEventLoop<ET>,
}
impl<ET: 'static + Clone> WinitEventLoop<ET> {
    /// Builds the event loop without creating a window before the host resumes.
    pub fn new(config: &WinitMapWindowConfig<ET>) -> Result<Self, WinitHostError> {
        Ok(Self {
            event_loop: config.event_loop()?,
        })
    }
    /// Creates a sender for this event loop.
    pub fn create_proxy(&self) -> WinitEventLoopProxy<ET> {
        WinitEventLoopProxy {
            proxy: self.event_loop.create_proxy(),
        }
    }

    /// Runs a native map factory on the active event loop and returns lifecycle failures.
    /// Suspended initialization preserves its map and services for the next resume.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn run_map_blocking<E, F, C>(
        self,
        config: WinitMapWindowConfig<ET>,
        factory: F,
        max_frames: Option<u64>,
    ) -> Result<(), WinitApplicationError<C>>
    where
        E: Environment<MapWindowConfig = WinitMapWindowConfig<ET>>,
        F: FnMut(WinitMapWindowConfig<ET>) -> Result<Map<E>, C>,
        C: std::error::Error + 'static,
    {
        let mut app = lifecycle::Application::new(config, factory, max_frames);
        self.event_loop
            .run_app(&mut app)
            .map_err(WinitHostError::EventLoop)?;
        app.finish()
    }

    /// Starts browser dispatch and resolves after the first successful renderer initialization
    /// with a handle that sends the map commands; each wakes the loop with `ET::default()`.
    /// Suspended initialization preserves its map and services for the next resume.
    #[cfg(target_arch = "wasm32")]
    pub async fn spawn_map<E, F, C>(
        self,
        config: WinitMapWindowConfig<ET>,
        factory: F,
        max_frames: Option<u64>,
    ) -> Result<MapHandle<E>, WinitApplicationError<C>>
    where
        E: Environment<MapWindowConfig = WinitMapWindowConfig<ET>>,
        F: FnMut(WinitMapWindowConfig<ET>) -> Result<Map<E>, C> + 'static,
        C: std::error::Error + 'static,
        ET: Default,
    {
        use winit::platform::web::EventLoopExtWebSys;
        let (callback, completion) = completion::channel();
        let mut app = lifecycle::Application::new(config, factory, max_frames);
        app.completion = Some(callback);
        let active_window = app.active_window.clone();
        let handle = app.handle(
            self.create_proxy()
                .waking_window(ET::default(), active_window),
        );
        self.event_loop.spawn_app(app);
        completion.await?;
        Ok(handle)
    }
}

/// Sender that reports when its host loop is closed.
pub struct WinitEventLoopProxy<ET: 'static> {
    proxy: RawEventLoopProxy<ET>,
}
impl<ET: 'static + Clone> WinitEventLoopProxy<ET> {
    /// Wakes the loop for a map handle by sending `event`, which the loop's handler passes
    /// to [`WinitApplication`]'s `user_event`.
    pub fn waking_with(self, event: ET) -> ProxyWake<ET> {
        #[cfg(target_arch = "wasm32")]
        {
            self.waking_window(event, std::rc::Rc::new(std::cell::RefCell::new(None)))
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            ProxyWake { proxy: self, event }
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn waking_window(self, event: ET, active_window: ActiveWindowSlot) -> ProxyWake<ET> {
        use std::{cell::Cell, rc::Rc};

        use wasm_bindgen::{closure::Closure, JsValue};

        let queued = Rc::new(Cell::new(false));
        let queued_reset = queued.clone();
        let fallback_window = active_window.clone();
        let proxy = self.proxy.clone();
        let ev = event.clone();
        let closure = Rc::new(Closure::wrap(Box::new(move |_val: JsValue| {
            queued_reset.set(false);
            let window = fallback_window
                .try_borrow()
                .ok()
                .and_then(|slot| slot.as_ref().and_then(std::sync::Weak::upgrade));
            let hidden = web_sys::window()
                .and_then(|w| w.document())
                .is_some_and(|doc| doc.hidden());
            if let (Some(window), false) = (window, hidden) {
                window.request_redraw();
            } else if proxy.send_event(ev.clone()).is_err() {
                tracing::debug!("the map's event loop closed before a command reached it");
            }
        }) as Box<dyn FnMut(JsValue)>));
        ProxyWake {
            proxy: self,
            event,
            active_window,
            queued,
            microtask: closure,
        }
    }
}

/// Wakes an idle event loop so it applies a map handle's commands.
pub struct ProxyWake<ET: 'static> {
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    proxy: WinitEventLoopProxy<ET>,
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    event: ET,
    #[cfg(target_arch = "wasm32")]
    active_window: ActiveWindowSlot,
    #[cfg(target_arch = "wasm32")]
    queued: std::rc::Rc<std::cell::Cell<bool>>,
    #[cfg(target_arch = "wasm32")]
    microtask: std::rc::Rc<wasm_bindgen::closure::Closure<dyn FnMut(wasm_bindgen::JsValue)>>,
}
impl<ET: 'static + Clone> WakeHost for ProxyWake<ET> {
    fn wake(&self) {
        #[cfg(target_arch = "wasm32")]
        {
            let window = self
                .active_window
                .try_borrow()
                .ok()
                .and_then(|slot| slot.as_ref().and_then(std::sync::Weak::upgrade));
            let hidden = web_sys::window()
                .and_then(|w| w.document())
                .is_some_and(|doc| doc.hidden());
            if let (Some(window), false) = (window, hidden) {
                // Requesting redraw on an active web window is idempotent, whereas
                // `EventLoopProxy::send_event` cancels pending `requestAnimationFrame` callbacks
                // in winit 0.30 and starves rendering under external VSync command loops.
                window.request_redraw();
            } else if !self.queued.replace(true) {
                drop(
                    js_sys::Promise::resolve(&wasm_bindgen::JsValue::UNDEFINED)
                        .then(&self.microtask),
                );
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        if self.proxy.send_event(self.event.clone()).is_err() {
            tracing::debug!("the map's event loop closed before a command reached it");
        }
    }
}

impl<ET: 'static> EventLoopProxy<ET> for WinitEventLoopProxy<ET> {
    fn send_event(&self, event: ET) -> Result<(), SendEventError> {
        self.proxy
            .send_event(event)
            .map_err(|_| SendEventError::Closed)
    }
}
