//! Dedicated `OffscreenCanvas` render-worker host for single-threaded and multithreaded WebGPU builds.

#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod input;
mod preinit;
mod wake;
mod window;

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use instant::Instant;
use maplibre::{
    input::{InputController, UpdateState},
    io::apc::AsyncProcedureCall,
    map::{handle::MapCommands, Map},
    render::frame_input::FrameInput,
};
use wasm_bindgen::{closure::Closure, prelude::*};
use web_sys::{DedicatedWorkerGlobalScope, MessagePort};

pub(crate) use self::window::OffscreenEnvironment;
use self::window::{register_runner_waker, unregister_runner_waker, SharedWindowState, WorkerApc};
use crate::{controller::MapController, platform::UsedOffscreenKernelEnvironment};

const MAX_INPUT_DT: Duration = Duration::from_millis(50);

fn apc_has_arrivals(apc: &WorkerApc) -> bool {
    <WorkerApc as AsyncProcedureCall<UsedOffscreenKernelEnvironment>>::has_arrivals(apc)
}

fn apc_supports_waker(apc: &WorkerApc) -> bool {
    <WorkerApc as AsyncProcedureCall<UsedOffscreenKernelEnvironment>>::supports_waker(apc)
}

type WakeTimerSlot = RefCell<Option<(i32, Closure<dyn FnMut()>)>>;
type WakeChannelSlot = RefCell<Option<(MessagePort, MessagePort, Closure<dyn FnMut()>)>>;
type RafClosureSlot = RefCell<Option<Closure<dyn FnMut(f64)>>>;

const MAX_BOOT_IMMEDIATE_PUMPS: u8 = 128;

struct RunnerInner {
    id: u64,
    scope: DedicatedWorkerGlobalScope,
    window_state: Rc<SharedWindowState>,
    map: Map<OffscreenEnvironment>,
    input: InputController,
    commands: MapCommands<OffscreenEnvironment>,
    started: Instant,
    last_render: Instant,
    raf_pending: Cell<bool>,
    cmd_pending: Cell<bool>,
    raf_handle: Cell<Option<i32>>,
    wake_timer: WakeTimerSlot,
    boot_wake_channel: WakeChannelSlot,
    immediate_wake_pending: Cell<bool>,
    raf_closure: RafClosureSlot,
    initial_load_settled: Cell<bool>,
    initial_pumps: Cell<u8>,
    last_style_swap: Cell<u64>,
    closed: Cell<bool>,
}

impl RunnerInner {
    fn sync_style_swap(&self) {
        let swap = self.map.style_swap_count();
        if swap != self.last_style_swap.get() {
            self.last_style_swap.set(swap);
            self.initial_load_settled.set(false);
            self.initial_pumps.set(0);
        }
    }
    fn schedule_commands(rc: &Rc<RefCell<Self>>) {
        let should_spawn = match rc.try_borrow() {
            Ok(inner) => {
                if inner.closed.get() || inner.cmd_pending.get() {
                    false
                } else {
                    inner.cmd_pending.set(true);
                    true
                }
            }
            Err(_) => true,
        };
        if !should_spawn {
            return;
        }
        let weak = Rc::downgrade(rc);
        wasm_bindgen_futures::spawn_local(async move {
            if let Some(strong) = weak.upgrade() {
                Self::drain_commands(&strong);
            }
        });
    }

    fn drain_commands(rc: &Rc<RefCell<Self>>) {
        let needs_frame = {
            let Ok(mut inner) = rc.try_borrow_mut() else {
                return;
            };
            inner.cmd_pending.set(false);
            if inner.closed.get() || inner.commands.is_closed() {
                inner.shutdown();
                return;
            }
            let s = &mut *inner;
            s.commands.apply(&mut s.map);
            s.sync_style_swap();
            !s.closed.get()
                && (s.map.has_ready_work()
                    || apc_has_arrivals(s.map.kernel().apc())
                    || s.input.moved_view())
        };
        if needs_frame {
            Self::schedule_wakeup(rc);
        }
    }

    fn take_boot_pump(&self, has_immediate_work: bool) -> bool {
        if self.initial_load_settled.get() || self.initial_pumps.get() >= MAX_BOOT_IMMEDIATE_PUMPS {
            return false;
        }
        if has_immediate_work || self.initial_pumps.get() <= 2 {
            self.initial_pumps
                .set(self.initial_pumps.get().saturating_add(1));
            true
        } else {
            false
        }
    }

    fn compute_next_wakeup(&self, after: Instant) -> (Option<Duration>, bool) {
        let map_ready = if apc_supports_waker(self.map.kernel().apc()) {
            self.map.has_ready_work()
        } else {
            self.map.needs_redraw()
        };
        let arrivals = apc_has_arrivals(self.map.kernel().apc());
        let input_moved = self.input.moved_view();
        let in_flight =
            <WorkerApc as AsyncProcedureCall<UsedOffscreenKernelEnvironment>>::in_flight_calls(
                self.map.kernel().apc(),
            );
        let wait = match (self.input.next_wait(after), self.map.next_retry_delay()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };

        if !self.initial_load_settled.get()
            && !self.map.is_loading()
            && !self.map.has_ready_work()
            && !self
                .map
                .context()
                .is_ok_and(maplibre::context::MapContext::has_pending_drapes)
            && !arrivals
            && in_flight == 0
            && wait.is_none()
        {
            self.initial_load_settled.set(true);
            <WorkerApc as AsyncProcedureCall<UsedOffscreenKernelEnvironment>>::set_boot_unthrottled(
                self.map.kernel().apc(),
                false,
            );
        }
        let next_delay = if map_ready || arrivals || input_moved || wait == Some(Duration::ZERO) {
            Some(Duration::ZERO)
        } else {
            wait
        };
        let boot_pump =
            next_delay == Some(Duration::ZERO) && self.take_boot_pump(arrivals || map_ready);
        (next_delay, boot_pump)
    }

    fn step_frame(&mut self) -> Option<(Option<Duration>, bool)> {
        self.raf_pending.set(false);
        self.raf_handle.set(None);
        self.clear_wake_timer();
        if self.closed.get() || self.commands.is_closed() {
            self.shutdown();
            return None;
        }

        self.commands.apply(&mut self.map);
        self.sync_style_swap();
        if self.commands.is_closed() {
            self.shutdown();
            return None;
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
        if let Err(err) = self.map.run_schedule() {
            log::error!("offscreen map schedule error: {err}");
        }
        self.commands.observe(&mut self.map);
        Some(self.compute_next_wakeup(Instant::now()))
    }

    fn run_frame(rc: &Rc<RefCell<Self>>) {
        let Some((next_delay, boot_pump)) =
            rc.try_borrow_mut().ok().and_then(|mut s| s.step_frame())
        else {
            return;
        };

        match (next_delay, boot_pump) {
            (Some(Duration::ZERO), true) => Self::schedule_immediate_pump(rc),
            (Some(Duration::ZERO), false) => Self::schedule_raf(rc),
            (Some(delay), _) => Self::schedule_wake(rc, delay),
            (None, _) => {
                if let Ok(inner) = rc.try_borrow() {
                    inner.clear_wake_timer();
                }
            }
        }
    }

    fn shutdown(&mut self) {
        if self.closed.replace(true) {
            return;
        }
        unregister_runner_waker(self.id);
        self.clear_wake_timer();
        if let Some((port1, port2, _)) = self.boot_wake_channel.borrow_mut().take() {
            port2.set_onmessage(None);
            port1.close();
            port2.close();
        }
        self.cancel_raf();
        *self.window_state.redraw.borrow_mut() = None;
        *self.raf_closure.borrow_mut() = None;
        self.commands.close();
    }
}

impl Drop for RunnerInner {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Worker-side owner driving an [`OffscreenCanvas`] map loop and input state machine.
#[wasm_bindgen]
pub struct OffscreenMapRunner {
    inner: Rc<RefCell<RunnerInner>>,
    controller: MapController,
}

fn start_runner(
    runner_id: u64,
    scope: DedicatedWorkerGlobalScope,
    window_state: Rc<SharedWindowState>,
    map: Map<OffscreenEnvironment>,
) -> OffscreenMapRunner {
    let now = Instant::now();
    let initial_style_swap = map.style_swap_count();
    let commands = MapCommands::<OffscreenEnvironment>::default();
    let inner = Rc::new(RefCell::new(RunnerInner {
        id: runner_id,
        scope,
        window_state: window_state.clone(),
        map,
        input: InputController::new(0.2, 100.0, 1.0),
        commands,
        started: now,
        last_render: now,
        raf_pending: Cell::new(false),
        cmd_pending: Cell::new(false),
        raf_handle: Cell::new(None),
        wake_timer: RefCell::new(None),
        boot_wake_channel: RefCell::new(None),
        immediate_wake_pending: Cell::new(false),
        raf_closure: RefCell::new(None),
        initial_load_settled: Cell::new(false),
        initial_pumps: Cell::new(0),
        last_style_swap: Cell::new(initial_style_swap),
        closed: Cell::new(false),
    }));

    let weak_for_redraw = Rc::downgrade(&inner);
    let redraw_rc: Rc<dyn Fn()> = Rc::new(move || {
        if let Some(strong) = weak_for_redraw.upgrade() {
            RunnerInner::schedule_wakeup(&strong);
        }
    });
    *window_state.redraw.borrow_mut() = Some(redraw_rc.clone());

    let apc_waker = register_runner_waker(runner_id, redraw_rc);
    if let Ok(guard) = inner.try_borrow() {
        <WorkerApc as AsyncProcedureCall<UsedOffscreenKernelEnvironment>>::set_waker(
            guard.map.kernel().apc(),
            apc_waker,
        );
    }

    let weak_for_commands = Rc::downgrade(&inner);
    let handle = inner.borrow().commands.handle(move || {
        if let Some(strong) = weak_for_commands.upgrade() {
            RunnerInner::schedule_commands(&strong);
        }
    });
    let controller = MapController::new_offscreen(handle);

    // Run the initial frame immediately so GPU pipelines warm and viewport tile HTTP requests
    // launch at t=0 without waiting up to one VSync for the first worker animation frame.
    RunnerInner::run_frame(&inner);
    OffscreenMapRunner { inner, controller }
}
