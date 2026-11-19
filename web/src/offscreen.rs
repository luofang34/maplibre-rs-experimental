//! Dedicated `OffscreenCanvas` render-worker host for single-threaded and multithreaded WebGPU builds.

#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod input;
mod preinit;
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
use wasm_bindgen::{closure::Closure, prelude::*, JsCast};
use web_sys::{DedicatedWorkerGlobalScope, MessageChannel, MessagePort};

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
    closed: Cell<bool>,
}

impl RunnerInner {
    fn clear_wake_timer(&self) {
        self.immediate_wake_pending.set(false);
        if let Some((id, _)) = self.wake_timer.borrow_mut().take() {
            self.scope.clear_timeout_with_handle(id);
        }
    }

    fn schedule_raf(rc: &Rc<RefCell<Self>>) {
        let should_schedule = {
            let Ok(inner) = rc.try_borrow() else {
                return;
            };
            if inner.closed.get() || inner.raf_pending.get() {
                false
            } else {
                inner.raf_pending.set(true);
                inner.clear_wake_timer();
                true
            }
        };
        if !should_schedule {
            return;
        }
        let Ok(inner) = rc.try_borrow() else {
            return;
        };
        if inner.raf_closure.borrow().is_none() {
            let weak = Rc::downgrade(rc);
            let closure = Closure::wrap(Box::new(move |_ts: f64| {
                if let Some(strong) = weak.upgrade() {
                    Self::run_frame(&strong);
                }
            }) as Box<dyn FnMut(f64)>);
            *inner.raf_closure.borrow_mut() = Some(closure);
        }
        let borrow = inner.raf_closure.borrow();
        let Some(closure) = borrow.as_ref() else {
            return;
        };
        let raf_fn =
            js_sys::Reflect::get(&inner.scope, &JsValue::from_str("requestAnimationFrame"))
                .ok()
                .and_then(|v| v.dyn_into::<js_sys::Function>().ok());
        if let Some(raf) = raf_fn {
            if let Ok(id) = raf.call1(&inner.scope, closure.as_ref().unchecked_ref()) {
                inner.raf_handle.set(id.as_f64().map(|n| n as i32));
                return;
            }
        }
        let _ = inner
            .scope
            .set_timeout_with_callback_and_timeout_and_arguments_0(
                closure.as_ref().unchecked_ref(),
                16,
            );
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
            !s.closed.get()
                && (s.map.has_ready_work()
                    || apc_has_arrivals(s.map.kernel().apc())
                    || s.input.moved_view())
        };
        if needs_frame {
            Self::schedule_wakeup(rc);
        }
    }

    fn take_boot_pump(&self, arrivals: bool) -> bool {
        if self.initial_load_settled.get() || self.initial_pumps.get() >= MAX_BOOT_IMMEDIATE_PUMPS {
            return false;
        }
        if arrivals || self.initial_pumps.get() <= 2 {
            self.initial_pumps
                .set(self.initial_pumps.get().saturating_add(1));
            true
        } else {
            false
        }
    }

    fn cancel_raf(&self) {
        self.raf_pending.set(false);
        if let Some(raf_id) = self.raf_handle.take() {
            if let Ok(cancel) =
                js_sys::Reflect::get(&self.scope, &JsValue::from_str("cancelAnimationFrame"))
            {
                if let Ok(func) = cancel.dyn_into::<js_sys::Function>() {
                    let _ = func.call1(&self.scope, &JsValue::from(raf_id));
                }
            }
        }
    }

    fn schedule_wakeup(rc: &Rc<RefCell<Self>>) {
        let pump_immediate = {
            let Ok(inner) = rc.try_borrow() else {
                return;
            };
            if inner.closed.get() || inner.immediate_wake_pending.get() {
                return;
            }
            let arrivals = apc_has_arrivals(inner.map.kernel().apc());
            if inner.raf_pending.get() {
                if !inner.initial_load_settled.get() && arrivals && inner.take_boot_pump(true) {
                    inner.cancel_raf();
                    true
                } else {
                    return;
                }
            } else if !inner.map.has_ready_work() && !arrivals && !inner.input.moved_view() {
                return;
            } else {
                inner.take_boot_pump(arrivals)
            }
        };
        if pump_immediate {
            Self::schedule_immediate_pump(rc);
        } else {
            Self::schedule_raf(rc);
        }
    }

    fn run_frame(rc: &Rc<RefCell<Self>>) {
        let (next_delay, boot_pump) = {
            let Ok(mut inner) = rc.try_borrow_mut() else {
                return;
            };
            inner.raf_pending.set(false);
            inner.raf_handle.set(None);
            inner.clear_wake_timer();
            if inner.closed.get() || inner.commands.is_closed() {
                inner.shutdown();
                return;
            }

            let s = &mut *inner;
            s.commands.apply(&mut s.map);
            if s.commands.is_closed() {
                s.shutdown();
                return;
            }

            let now = Instant::now();
            let elapsed = (now - s.last_render).min(MAX_INPUT_DT);
            s.last_render = now;
            if let Ok(context) = s.map.context_mut() {
                context
                    .world
                    .resources
                    .get_or_init_mut::<FrameInput>()
                    .timestamp = now - s.started;
                s.input.update_state(context, elapsed);
            }
            if let Err(err) = s.map.run_schedule() {
                log::error!("offscreen map schedule error: {err}");
            }
            s.commands.observe(&mut s.map);

            let after = Instant::now();
            let map_ready = if apc_supports_waker(s.map.kernel().apc()) {
                s.map.has_ready_work()
            } else {
                s.map.needs_redraw()
            };
            let arrivals = apc_has_arrivals(s.map.kernel().apc());
            let input_moved = s.input.moved_view();
            let in_flight =
                <WorkerApc as AsyncProcedureCall<UsedOffscreenKernelEnvironment>>::in_flight_calls(
                    s.map.kernel().apc(),
                );
            let wait = match (s.input.next_wait(after), s.map.next_retry_delay()) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            };

            if !s.initial_load_settled.get()
                && !s.map.is_loading()
                && !arrivals
                && in_flight == 0
                && wait.is_none()
            {
                s.initial_load_settled.set(true);
                <WorkerApc as AsyncProcedureCall<UsedOffscreenKernelEnvironment>>::set_boot_unthrottled(
                    s.map.kernel().apc(),
                    false,
                );
            }
            let next_delay = if map_ready || arrivals || input_moved || wait == Some(Duration::ZERO)
            {
                Some(Duration::ZERO)
            } else {
                wait
            };
            let boot_pump = next_delay == Some(Duration::ZERO) && s.take_boot_pump(arrivals);
            (next_delay, boot_pump)
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

    fn schedule_wake(rc: &Rc<RefCell<Self>>, delay: Duration) {
        Self::schedule_timer(rc, delay, Self::schedule_raf);
    }

    fn schedule_immediate_pump(rc: &Rc<RefCell<Self>>) {
        let should_post = {
            let Ok(inner) = rc.try_borrow() else {
                return;
            };
            if inner.closed.get() || inner.raf_pending.get() || inner.immediate_wake_pending.get() {
                return;
            }
            inner.clear_wake_timer();
            if inner.boot_wake_channel.borrow().is_none() {
                if let Ok(channel) = MessageChannel::new() {
                    let port1 = channel.port1();
                    let port2 = channel.port2();
                    let weak = Rc::downgrade(rc);
                    let closure = Closure::wrap(Box::new(move || {
                        if let Some(strong) = weak.upgrade() {
                            let still_pending = strong
                                .try_borrow()
                                .is_ok_and(|s| !s.closed.get() && s.immediate_wake_pending.get());
                            if still_pending {
                                Self::run_frame(&strong);
                            }
                        }
                    }) as Box<dyn FnMut()>);
                    port2.set_onmessage(Some(closure.as_ref().unchecked_ref()));
                    *inner.boot_wake_channel.borrow_mut() = Some((port1, port2, closure));
                }
            }
            if let Some((port1, _, _)) = inner.boot_wake_channel.borrow().as_ref() {
                if port1.post_message(&JsValue::UNDEFINED).is_ok() {
                    inner.immediate_wake_pending.set(true);
                    false
                } else {
                    true
                }
            } else {
                true
            }
        };
        if should_post {
            Self::schedule_timer(rc, Duration::ZERO, Self::run_frame);
            if let Ok(inner) = rc.try_borrow() {
                if inner.wake_timer.borrow().is_some() {
                    inner.immediate_wake_pending.set(true);
                }
            }
        }
    }

    fn schedule_timer(
        rc: &Rc<RefCell<Self>>,
        delay: Duration,
        callback_fn: fn(&Rc<RefCell<Self>>),
    ) {
        let Ok(inner) = rc.try_borrow() else {
            return;
        };
        if inner.closed.get() || inner.raf_pending.get() {
            return;
        }
        inner.clear_wake_timer();
        let weak = Rc::downgrade(rc);
        let callback = Closure::wrap(Box::new(move || {
            if let Some(strong) = weak.upgrade() {
                callback_fn(&strong);
            }
        }) as Box<dyn FnMut()>);
        let millis = i32::try_from(delay.as_millis()).unwrap_or(i32::MAX);
        if let Ok(id) = inner
            .scope
            .set_timeout_with_callback_and_timeout_and_arguments_0(
                callback.as_ref().unchecked_ref(),
                millis,
            )
        {
            *inner.wake_timer.borrow_mut() = Some((id, callback));
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
