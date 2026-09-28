//! Event dispatch and redraw timing for winit windows.

use std::fmt::Debug;

use instant::Instant;
use maplibre::{
    environment::Environment,
    event_loop::{EventLoop, EventLoopError, EventLoopProxy, SendEventError},
    map::Map,
    render::frame_input::FrameInput,
    window::{HeadedMapWindow, MapWindowConfig, PhysicalSize},
};
use winit::{
    event::{ElementState, Event, KeyEvent, WindowEvent},
    event_loop::ActiveEventLoop,
    keyboard::{Key, NamedKey},
};

use crate::input::{InputController, UpdateState};

pub type RawWinitEventLoop<ET> = winit::event_loop::EventLoop<ET>;
pub type RawEventLoopProxy<ET> = winit::event_loop::EventLoopProxy<ET>;

pub struct WinitEventLoop<ET: 'static> {
    pub(crate) event_loop: RawWinitEventLoop<ET>,
}

impl<ET: 'static + PartialEq + Debug> EventLoop<ET> for WinitEventLoop<ET> {
    type EventLoopProxy = WinitEventLoopProxy<ET>;

    fn run<E>(self, map: Map<E>, max_frames: Option<u64>) -> Result<(), EventLoopError>
    where
        E: Environment,
        <E::MapWindowConfig as MapWindowConfig>::MapWindow: HeadedMapWindow,
    {
        let mut state = Dispatch::new(map, max_frames);
        let dispatch = move |event, target: &ActiveEventLoop| state.handle_event(event, target);
        #[cfg(target_arch = "wasm32")]
        {
            winit::platform::web::EventLoopExtWebSys::spawn(self.event_loop, dispatch);
            Ok(())
        }
        #[cfg(not(target_arch = "wasm32"))]
        self.event_loop.run(dispatch).map_err(|_| EventLoopError)
    }

    fn create_proxy(&self) -> Self::EventLoopProxy {
        WinitEventLoopProxy {
            proxy: self.event_loop.create_proxy(),
        }
    }
}

struct Dispatch<E: Environment>
where
    <E::MapWindowConfig as MapWindowConfig>::MapWindow: HeadedMapWindow,
{
    map: Map<E>,
    input: InputController,
    started: Instant,
    last_render: Instant,
    frame: u64,
    max_frames: Option<u64>,
    scale_factor: f64,
}

impl<E: Environment> Dispatch<E>
where
    <E::MapWindowConfig as MapWindowConfig>::MapWindow: HeadedMapWindow,
{
    fn new(map: Map<E>, max_frames: Option<u64>) -> Self {
        let now = Instant::now();
        let scale_factor = map.window().scale_factor();
        Self {
            map,
            input: InputController::new(0.2, 100.0, 0.1),
            started: now,
            last_render: now,
            frame: 0,
            max_frames,
            scale_factor,
        }
    }

    fn handle_event<ET>(&mut self, event: Event<ET>, target: &ActiveEventLoop) {
        #[cfg(target_os = "android")]
        if !self.map.is_initialized() && matches!(event, Event::Resumed) {
            let result = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(self.map.initialize_renderer())
            });
            if let Err(error) = result {
                tracing::error!(%error, "resuming renderer failed");
                target.exit();
            }
            return;
        }
        match event {
            Event::DeviceEvent { event, .. } => {
                self.input.device_input(&event);
            }
            Event::WindowEvent { event, window_id }
                if window_id == self.map.window().id().into() =>
            {
                self.window_event(&event, target);
            }
            Event::Suspended => self.map.reset(),
            _ => {}
        }
    }

    fn window_event(&mut self, event: &WindowEvent, target: &ActiveEventLoop) {
        if matches!(event, WindowEvent::RedrawRequested) {
            self.redraw(target);
        }
        if self.input.window_input(event, self.scale_factor) {
            return;
        }
        match event {
            WindowEvent::CloseRequested
            | WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state: ElementState::Pressed,
                        logical_key: Key::Named(NamedKey::Escape),
                        ..
                    },
                ..
            } => target.exit(),
            WindowEvent::Resized(size) => {
                // Minimized windows can report a zero extent, which cannot configure a surface.
                if let Some(size) = PhysicalSize::new(size.width, size.height) {
                    if let Ok(context) = self.map.context_mut() {
                        context.resize(size, self.scale_factor);
                        self.map.window().request_redraw();
                    }
                }
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale_factor = *scale_factor;
                if let Ok(context) = self.map.context_mut() {
                    context.resize(context.renderer.resources.surface.size(), self.scale_factor);
                }
            }
            _ => {}
        }
    }

    fn redraw(&mut self, target: &ActiveEventLoop) {
        if !self.map.is_initialized() {
            return;
        }
        let now = Instant::now();
        let elapsed = now - self.last_render;
        self.last_render = now;
        if let Ok(context) = self.map.context_mut() {
            context
                .world
                .resources
                .get_or_init_mut::<FrameInput>()
                .timestamp = now - self.started;
            self.input.update_state(context, elapsed);
        }
        if let Err(error) = self.map.run_schedule() {
            tracing::error!(%error, "rendering frame failed");
            target.exit();
            return;
        }
        if self.max_frames.is_some_and(|max| self.frame >= max) {
            target.exit();
        }
        self.frame = self.frame.wrapping_add(1);
        self.map.window().request_redraw();
    }
}

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
