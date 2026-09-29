//! The handler owns pending initialization so suspension drops every surface synchronously.
use super::dispatch::Dispatch;
use crate::{RawWinitWindow, WinitApplicationError, WinitMapWindowConfig};
use maplibre::{environment::Environment, map::Map, window::HeadedMapWindow};
use std::{
    cell::RefCell,
    future::Future,
    pin::Pin,
    rc::Rc,
    sync::{Arc, Weak},
    task::{Context, Poll, Wake, Waker},
};
use winit::{
    application::ApplicationHandler,
    event::{DeviceEvent, DeviceId, WindowEvent},
    event_loop::ActiveEventLoop,
    window::WindowId,
};

type Initializing<E> = Pin<Box<dyn Future<Output = Result<Dispatch<E>, maplibre::map::MapError>>>>;
pub(super) type Completion<C> = Box<dyn FnOnce(Result<(), WinitApplicationError<C>>)>;

enum State<E: Environment>
where
    <E::MapWindowConfig as maplibre::window::MapWindowConfig>::MapWindow: HeadedMapWindow,
{
    Dormant,
    Initializing(Initializing<E>),
    Ready(Box<Dispatch<E>>),
    Suspended(Box<Dispatch<E>>),
}

/// A map handler that creates windows on resume and cancels GPU initialization on suspend.
/// Hosts may delegate winit callbacks to this handler when integrating their own event loop.
pub struct Application<E, F, ET, C>
where
    E: Environment<MapWindowConfig = WinitMapWindowConfig<ET>>,
    ET: 'static + Clone,
    C: std::error::Error + 'static,
{
    config: WinitMapWindowConfig<ET>,
    factory: F,
    state: State<E>,
    recovery: Rc<RefCell<Option<Dispatch<E>>>>,
    window: Weak<RawWinitWindow>,
    max_frames: Option<u64>,
    active: bool,
    initialized: bool,
    error: Option<WinitApplicationError<C>>,
    pub(super) completion: Option<Completion<C>>,
}

impl<E, F, ET, C> Application<E, F, ET, C>
where
    E: Environment<MapWindowConfig = WinitMapWindowConfig<ET>>,
    ET: 'static + Clone,
    F: FnMut(WinitMapWindowConfig<ET>) -> Result<Map<E>, C>,
    C: std::error::Error + 'static,
{
    /// Defers window creation and map construction until the first resumed callback.
    pub fn new(config: WinitMapWindowConfig<ET>, factory: F, max_frames: Option<u64>) -> Self {
        Self {
            config,
            factory,
            state: State::Dormant,
            recovery: Rc::new(RefCell::new(None)),
            window: Weak::new(),
            max_frames,
            active: false,
            initialized: false,
            error: None,
            completion: None,
        }
    }
    /// Returns any initialization or frame error after the host event loop exits.
    pub fn finish(mut self) -> Result<(), WinitApplicationError<C>> {
        match self.error.take() {
            Some(error) => Err(error),
            None if self.initialized => Ok(()),
            None => Err(WinitApplicationError::ClosedBeforeReady),
        }
    }
    fn fail(&mut self, active: &ActiveEventLoop, error: WinitApplicationError<C>) {
        tracing::error!(%error,"windowed map lifecycle failed");
        self.state = State::Dormant;
        drop(self.recovery.borrow_mut().take());
        if let Some(completion) = self.completion.take() {
            completion(Err(error));
        } else {
            self.error = Some(error);
        }
        active.exit();
    }
    fn begin(&mut self, active: &ActiveEventLoop) -> Result<(), WinitApplicationError<C>> {
        let state = std::mem::replace(&mut self.state, State::Dormant);
        let dispatch = match state {
            State::Suspended(dispatch) => *dispatch,
            State::Dormant => {
                let window = self.config.create_window(active)?;
                let config = self.config.clone().with_window(&window);
                let map = (self.factory)(config).map_err(WinitApplicationError::CreateMap)?;
                self.window = Arc::downgrade(map.window().handle());
                Dispatch::new(map, self.max_frames)
            }
            other => {
                self.state = other;
                return Ok(());
            }
        };
        let initialization =
            super::initialization::Initialization::new(dispatch, self.recovery.clone());
        self.state = State::Initializing(Box::pin(initialization.run()));
        if let Some(window) = self.window.upgrade() {
            window.request_redraw();
        }
        Ok(())
    }
    fn poll_initialization(&mut self, active: &ActiveEventLoop) {
        let State::Initializing(future) = &mut self.state else {
            return;
        };
        let waker = Waker::from(Arc::new(WindowWake(self.window.clone())));
        let Poll::Ready(result) = future.as_mut().poll(&mut Context::from_waker(&waker)) else {
            return;
        };
        let mut dispatch = match result {
            Ok(dispatch) => dispatch,
            Err(error) => {
                self.fail(active, WinitApplicationError::Initialize(error));
                return;
            }
        };
        dispatch.sync_size();
        self.state = State::Ready(Box::new(dispatch));
        self.initialized = true;
        if let Some(completion) = self.completion.take() {
            completion(Ok(()));
        }
    }
}

impl<E, F, ET, C> ApplicationHandler<ET> for Application<E, F, ET, C>
where
    E: Environment<MapWindowConfig = WinitMapWindowConfig<ET>>,
    ET: 'static + Clone,
    F: FnMut(WinitMapWindowConfig<ET>) -> Result<Map<E>, C>,
    C: std::error::Error + 'static,
{
    fn resumed(&mut self, active: &ActiveEventLoop) {
        self.active = true;
        if let Err(error) = self.begin(active) {
            self.fail(active, error);
        }
    }
    fn suspended(&mut self, _active: &ActiveEventLoop) {
        self.active = false;
        let state = std::mem::replace(&mut self.state, State::Dormant);
        self.state = match state {
            State::Ready(mut dispatch) => {
                dispatch.map.reset();
                State::Suspended(dispatch)
            }
            State::Suspended(dispatch) => State::Suspended(dispatch),
            // Dropping the owned future releases partially created surfaces before returning.
            State::Initializing(future) => {
                drop(future);
                self.recovery
                    .borrow_mut()
                    .take()
                    .map_or(State::Dormant, |dispatch| {
                        State::Suspended(Box::new(dispatch))
                    })
            }
            State::Dormant => State::Dormant,
        };
    }
    fn window_event(&mut self, active: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        if id != window.id() {
            return;
        }
        if matches!(event, WindowEvent::CloseRequested) {
            active.exit();
            return;
        }
        if !self.active {
            return;
        }
        if matches!(event, WindowEvent::RedrawRequested) {
            self.poll_initialization(active);
            let size = window.inner_size();
            if size.width == 0 || size.height == 0 {
                return;
            }
            if let State::Ready(dispatch) = &mut self.state {
                if let Err(error) = dispatch.redraw(active) {
                    self.fail(active, WinitApplicationError::Frame(error));
                }
            }
        } else if let State::Ready(dispatch) = &mut self.state {
            dispatch.window_event(&event, active);
        }
    }
    fn device_event(&mut self, _active: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if self.active {
            if let State::Ready(dispatch) = &mut self.state {
                dispatch.device_event(&event);
            }
        }
    }
    fn exiting(&mut self, _active: &ActiveEventLoop) {
        self.state = State::Dormant;
        drop(self.recovery.borrow_mut().take());
        if let Some(completion) = self.completion.take() {
            completion(Err(WinitApplicationError::ClosedBeforeReady));
        }
    }
}

struct WindowWake(Weak<RawWinitWindow>);
impl Wake for WindowWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        if let Some(window) = self.0.upgrade() {
            window.request_redraw();
        }
    }
}
