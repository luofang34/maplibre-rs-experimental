//! Input and frames for an initialized map.
use crate::input::{InputController, UpdateState};
use instant::Instant;
use maplibre::{
    environment::Environment,
    map::Map,
    render::frame_input::FrameInput,
    window::{HeadedMapWindow, MapWindow, MapWindowConfig, PhysicalSize},
};
use winit::{
    event::{ElementState, KeyEvent, WindowEvent},
    event_loop::ActiveEventLoop,
    keyboard::{Key, NamedKey},
};

pub(super) struct Dispatch<E: Environment>
where
    <E::MapWindowConfig as MapWindowConfig>::MapWindow: HeadedMapWindow,
{
    pub(super) map: Map<E>,
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
    pub(super) fn new(map: Map<E>, max_frames: Option<u64>) -> Self {
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

    pub(super) fn device_event(&mut self, event: &winit::event::DeviceEvent) {
        self.input.device_input(event);
    }

    pub(super) fn sync_size(&mut self) {
        self.scale_factor = self.map.window().scale_factor();
        let size = self.map.window().size();
        if let Ok(context) = self.map.context_mut() {
            context.resize(size, self.scale_factor);
        }
        self.last_render = Instant::now();
    }

    pub(super) fn window_event(&mut self, event: &WindowEvent, target: &ActiveEventLoop) {
        if matches!(event, WindowEvent::RedrawRequested) {
            return;
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
                let size = self.map.window().size();
                if let Ok(context) = self.map.context_mut() {
                    context.resize(size, self.scale_factor);
                }
            }
            _ => {}
        }
    }

    pub(super) fn redraw(
        &mut self,
        target: &ActiveEventLoop,
    ) -> Result<(), maplibre::map::MapError> {
        if self.max_frames == Some(0) {
            target.exit();
            return Ok(());
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
        self.map.run_schedule()?;
        self.frame = self.frame.wrapping_add(1);
        if self.max_frames.is_some_and(|max| self.frame >= max) {
            target.exit();
        }
        self.map.window().request_redraw();
        Ok(())
    }
}
