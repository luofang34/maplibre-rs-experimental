//! Input and frames for an initialized map.
use std::time::Duration;

use instant::Instant;
use maplibre::{
    environment::Environment,
    io::apc::AsyncProcedureCall,
    map::Map,
    render::frame_input::FrameInput,
    window::{HeadedMapWindow, MapWindow, MapWindowConfig, PhysicalSize},
};
use winit::{
    event::{ElementState, KeyEvent, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow},
    keyboard::{Key, NamedKey},
};

use crate::input::{InputController, UpdateState};

/// Largest frame step handed to input handlers so an on-demand wake after a long idle period
/// applies one normal frame step instead of integrating the idle gap.
const MAX_INPUT_DT: Duration = Duration::from_millis(50);

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
    wake_at: Option<Instant>,
    #[cfg(target_arch = "wasm32")]
    window: std::sync::Weak<crate::RawWinitWindow>,
    #[cfg(target_arch = "wasm32")]
    wake_timer: Option<(i32, wasm_bindgen::closure::Closure<dyn FnMut()>)>,
}

impl<E: Environment> Dispatch<E>
where
    <E::MapWindowConfig as MapWindowConfig>::MapWindow: HeadedMapWindow,
{
    pub(super) fn new(
        map: Map<E>,
        max_frames: Option<u64>,
        #[cfg_attr(not(target_arch = "wasm32"), allow(unused_variables))] window: std::sync::Weak<
            crate::RawWinitWindow,
        >,
    ) -> Self {
        let now = Instant::now();
        let scale_factor = map.window().scale_factor();
        Self {
            map,
            input: InputController::new(0.2, 100.0, 1.0),
            started: now,
            last_render: now,
            frame: 0,
            max_frames,
            scale_factor,
            wake_at: None,
            #[cfg(target_arch = "wasm32")]
            window,
            #[cfg(target_arch = "wasm32")]
            wake_timer: None,
        }
    }

    pub(super) fn clear_wake_timer(&mut self) {
        self.wake_at = None;
        #[cfg(target_arch = "wasm32")]
        if let Some((handle, _)) = self.wake_timer.take() {
            if let Some(window) = web_sys::window() {
                window.clear_timeout_with_handle(handle);
            }
        }
    }

    pub(super) fn poll_wake(&mut self, target: &ActiveEventLoop) {
        let Some(wake_at) = self.wake_at else {
            return;
        };
        let now = Instant::now();
        if now >= wake_at {
            self.clear_wake_timer();
            target.set_control_flow(ControlFlow::Wait);
            self.map.window().request_redraw();
        } else {
            #[cfg(not(target_arch = "wasm32"))]
            target.set_control_flow(ControlFlow::WaitUntil(
                std::time::Instant::now() + (wake_at - now),
            ));
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
            // The gesture is applied when the next frame updates the view.
            self.map.window().request_redraw();
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
                    self.map.window().request_redraw();
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
        let elapsed = (now - self.last_render).min(MAX_INPUT_DT);
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
            self.clear_wake_timer();
            target.exit();
            return Ok(());
        }
        let after = Instant::now();
        let next = NextFrame {
            map: self.map.needs_redraw(),
            arrivals: self.map.kernel().apc().has_arrivals(),
            input: self.input.moved_view(),
            counting_frames: self.max_frames.is_some(),
            wait: self.input.next_wait(after),
        };
        // An idle map draws nothing until an event or a pending timer asks for a frame.
        match next.action() {
            NextAction::Redraw => {
                self.clear_wake_timer();
                target.set_control_flow(ControlFlow::Wait);
                self.map.window().request_redraw();
            }
            NextAction::Wait(delay) => self.schedule_wake(target, after, delay),
            NextAction::Sleep => {
                self.clear_wake_timer();
                target.set_control_flow(ControlFlow::Wait);
            }
        }
        Ok(())
    }

    fn schedule_wake(&mut self, target: &ActiveEventLoop, now: Instant, delay: Duration) {
        self.clear_wake_timer();
        self.wake_at = Some(now + delay);
        #[cfg(not(target_arch = "wasm32"))]
        target.set_control_flow(ControlFlow::WaitUntil(std::time::Instant::now() + delay));
        #[cfg(target_arch = "wasm32")]
        {
            target.set_control_flow(ControlFlow::Wait);
            if let Some(browser) = web_sys::window() {
                use wasm_bindgen::JsCast;
                let handle = self.window.clone();
                let callback = wasm_bindgen::closure::Closure::wrap(Box::new(move || {
                    if let Some(window) = handle.upgrade() {
                        window.request_redraw();
                    }
                })
                    as Box<dyn FnMut()>);
                let millis = i32::try_from(delay.as_millis()).unwrap_or(i32::MAX);
                if let Ok(id) = browser.set_timeout_with_callback_and_timeout_and_arguments_0(
                    callback.as_ref().unchecked_ref(),
                    millis,
                ) {
                    self.wake_timer = Some((id, callback));
                }
            }
        }
    }
}

impl<E: Environment> Drop for Dispatch<E>
where
    <E::MapWindowConfig as MapWindowConfig>::MapWindow: HeadedMapWindow,
{
    fn drop(&mut self) {
        self.clear_wake_timer();
    }
}

/// How the event loop proceeds after finishing a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum NextAction {
    /// Wait indefinitely for a window or worker event.
    #[default]
    Sleep,
    /// Sleep until a pending timer expires, then request a redraw.
    Wait(Duration),
    /// Request the next frame immediately.
    Redraw,
}

/// What decides whether the event loop asks for another frame after drawing one.
#[derive(Clone, Copy, Debug, Default)]
struct NextFrame {
    /// The map changed, animates or still loads since the frame it drew.
    map: bool,
    /// Worker results arrived that the next frame applies.
    arrivals: bool,
    /// Input moved the view, which inertia or a held key carries into the next frame.
    input: bool,
    /// The host asked for a number of frames, which are drawn whether or not anything changes.
    counting_frames: bool,
    /// Shortest pending input timer (such as a held wheel step or gesture-end release).
    wait: Option<Duration>,
}

impl NextFrame {
    fn action(self) -> NextAction {
        if self.map
            || self.arrivals
            || self.input
            || self.counting_frames
            || self.wait == Some(Duration::ZERO)
        {
            NextAction::Redraw
        } else if let Some(delay) = self.wait {
            NextAction::Wait(delay)
        } else {
            NextAction::Sleep
        }
    }
}

#[cfg(test)]
mod tests;
