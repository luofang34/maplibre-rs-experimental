//! Resumed window creation and event dispatch for native and browser hosts.
use crate::{WinitApplicationError, WinitHostError, WinitMapWindowConfig};
use maplibre::{
    environment::Environment,
    event_loop::{EventLoopProxy, SendEventError},
    map::Map,
};

mod dispatch;
mod lifecycle;
pub use lifecycle::Application as WinitApplication;
#[cfg(target_arch = "wasm32")]
mod completion;
mod initialization;

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

    /// Starts browser dispatch and resolves after the first successful renderer initialization.
    /// Suspended initialization preserves its map and services for the next resume.
    #[cfg(target_arch = "wasm32")]
    pub async fn spawn_map<E, F, C>(
        self,
        config: WinitMapWindowConfig<ET>,
        factory: F,
        max_frames: Option<u64>,
    ) -> Result<(), WinitApplicationError<C>>
    where
        E: Environment<MapWindowConfig = WinitMapWindowConfig<ET>>,
        F: FnMut(WinitMapWindowConfig<ET>) -> Result<Map<E>, C> + 'static,
        C: std::error::Error + 'static,
    {
        use winit::platform::web::EventLoopExtWebSys;
        let (callback, completion) = completion::channel();
        let mut app = lifecycle::Application::new(config, factory, max_frames);
        app.completion = Some(callback);
        self.event_loop.spawn_app(app);
        completion.await
    }
}

/// Sender that reports when its host loop is closed.
pub struct WinitEventLoopProxy<ET: 'static> {
    proxy: RawEventLoopProxy<ET>,
}
impl<ET: 'static> EventLoopProxy<ET> for WinitEventLoopProxy<ET> {
    fn send_event(&self, event: ET) -> Result<(), SendEventError> {
        self.proxy
            .send_event(event)
            .map_err(|_| SendEventError::Closed)
    }
}
