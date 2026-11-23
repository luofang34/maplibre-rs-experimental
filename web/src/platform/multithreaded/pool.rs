//! Web worker pool for shared-memory (`wasm32 + atomics`) execution with a shared work-stealing
//! queue (`Arc<SharedWorkQueue>`) and coalesced cross-realm wake notifications.
#![expect(
    unsafe_code,
    reason = "frees boxed Work ticket when posting message to worker fails"
)]

mod queue;

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use wasm_bindgen::{prelude::*, JsCast};
use web_sys::{DedicatedWorkerGlobalScope, MessageEvent, MessagePort, Worker};

pub use self::queue::PinnedFuture;
use self::queue::{ClaimOutcome, SharedWorkQueue};
use crate::error::WebError;

type NewWorker = Box<dyn Fn() -> Result<Worker, WebError>>;
type NewPort = Box<dyn Fn() -> Result<Option<MessagePort>, WebError>>;

const MAX_RECYCLED_WORKERS: usize = 8;
const RAMP_BACKLOG_THRESHOLD: usize = 2;
const RAMP_BOOT_GRACE_MS: u64 = 150;
const RAMP_COOLDOWN_MS: u64 = 100;
#[cfg(target_arch = "wasm32")]
const RAMP_MAX_MEMORY_PAGES: usize = 32 * 16;

thread_local! {
    static RECYCLED_WORKERS: RefCell<Vec<Worker>> = const { RefCell::new(Vec::new()) };
    static CURRENT_WORKER_WAKE: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

fn low_memory_for_ramp() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        core::arch::wasm32::memory_size::<0>() < RAMP_MAX_MEMORY_PAGES
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        true
    }
}

#[derive(Clone)]
enum WorkerEndpoint {
    Dedicated(Worker),
    Port(MessagePort),
}

impl WorkerEndpoint {
    fn as_js_value(&self) -> &JsValue {
        match self {
            Self::Dedicated(worker) => worker.as_ref(),
            Self::Port(port) => port.as_ref(),
        }
    }

    fn set_onmessage(&self, callback: Option<&js_sys::Function>) {
        match self {
            Self::Dedicated(worker) => worker.set_onmessage(callback),
            Self::Port(port) => {
                port.set_onmessage(callback);
                if callback.is_some() {
                    port.start();
                }
            }
        }
    }

    fn post_message(&self, message: &JsValue) -> Result<(), JsValue> {
        match self {
            Self::Dedicated(worker) => worker.post_message(message),
            Self::Port(port) => port.post_message(message),
        }
    }
}

#[cfg_attr(
    not(feature = "winit-window"),
    expect(
        dead_code,
        reason = "Dedicated worker factory is only constructed under winit-window"
    )
)]
enum EndpointFactory {
    Dedicated(NewWorker),
    Port(NewPort),
}

pub struct WorkerPool {
    factory: EndpointFactory,
    state: Rc<PoolState>,
}

type WorkerListener = Closure<dyn FnMut(MessageEvent)>;

struct PoolState {
    workers: RefCell<Vec<(WorkerEndpoint, Arc<AtomicBool>)>>,
    queue: Arc<SharedWorkQueue>,
    waker: RefCell<Option<Arc<dyn Fn() + Send + Sync>>>,
    listeners: RefCell<Vec<WorkerListener>>,
    max_workers: usize,
    factory_exhausted: Cell<bool>,
    created_at: instant::Instant,
    last_ramp_at: Cell<Option<instant::Instant>>,
}

impl PoolState {
    fn push(
        &self,
        endpoint: WorkerEndpoint,
        wake_pending: Arc<AtomicBool>,
        onmessage: WorkerListener,
    ) -> Option<usize> {
        let mut workers = self.workers.borrow_mut();
        if workers
            .iter()
            .any(|(existing, _)| existing.as_js_value() == endpoint.as_js_value())
        {
            return None;
        }
        endpoint.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        let idx = workers.len();
        workers.push((endpoint, wake_pending));
        self.listeners.borrow_mut().push(onmessage);
        Some(idx)
    }
}

impl Drop for PoolState {
    fn drop(&mut self) {
        self.queue.close_and_clear();
        let drained = std::mem::take(self.workers.get_mut());
        self.listeners.get_mut().clear();
        RECYCLED_WORKERS.with(|bin| {
            let mut bin = bin.borrow_mut();
            for (endpoint, _) in drained {
                endpoint.set_onmessage(None);
                match endpoint {
                    WorkerEndpoint::Dedicated(worker) => {
                        if bin.len() < MAX_RECYCLED_WORKERS {
                            bin.push(worker);
                        } else {
                            worker.terminate();
                        }
                    }
                    WorkerEndpoint::Port(port) => {
                        port.close();
                    }
                }
            }
        });
    }
}

pub struct Work {
    queue: Arc<SharedWorkQueue>,
    worker_index: usize,
    wake_pending: Arc<AtomicBool>,
}

impl Work {
    pub async fn run(self) {
        loop {
            let Some(func) = self.queue.pop_or_release(self.worker_index) else {
                break;
            };
            CURRENT_WORKER_WAKE.with(|slot| {
                *slot.borrow_mut() = Some(self.wake_pending.clone());
            });
            func().await;
        }
        #[cfg(target_arch = "wasm32")]
        crate::platform::alloc::drain_thread_magazine();
    }
}

fn post_worker_wake(scope: &DedicatedWorkerGlobalScope) -> Result<(), JsValue> {
    if let Ok(port_val) = js_sys::Reflect::get(scope, &JsValue::from_str("__maplibreRenderPort")) {
        if let Ok(port) = port_val.dyn_into::<MessagePort>() {
            return port.post_message(&JsValue::TRUE);
        }
    }
    scope.post_message(&JsValue::TRUE)
}

impl WorkerPool {
    /// Creates a `WorkerPool` primed with `initial` dedicated web workers and up to `max_workers`.
    #[cfg_attr(
        not(feature = "winit-window"),
        expect(
            dead_code,
            reason = "Dedicated worker constructor is only used under winit-window"
        )
    )]
    pub fn new(
        initial: usize,
        max_workers: usize,
        new_worker: NewWorker,
    ) -> Result<Self, WebError> {
        Self::with_factory(initial, max_workers, EndpointFactory::Dedicated(new_worker))
    }

    /// Creates a `WorkerPool` primed with `initial` `MessagePort`s and lazy growth up to
    /// `max_workers`.
    pub fn from_ports(
        initial: usize,
        max_workers: usize,
        new_port: NewPort,
    ) -> Result<Self, WebError> {
        Self::with_factory(initial, max_workers, EndpointFactory::Port(new_port))
    }

    pub(crate) fn worker_count(&self) -> usize {
        self.state.workers.borrow().len()
    }

    fn with_factory(
        initial: usize,
        max_workers: usize,
        factory: EndpointFactory,
    ) -> Result<WorkerPool, WebError> {
        let max_workers = max_workers.max(initial).max(1);
        let pool = WorkerPool {
            factory,
            state: Rc::new(PoolState {
                workers: RefCell::new(Vec::with_capacity(max_workers)),
                queue: Arc::new(SharedWorkQueue::new()),
                waker: RefCell::new(None),
                listeners: RefCell::new(Vec::with_capacity(max_workers)),
                max_workers,
                factory_exhausted: Cell::new(false),
                created_at: instant::Instant::now(),
                last_ramp_at: Cell::new(None),
            }),
        };
        for _ in 0..initial {
            if pool.try_spawn()?.is_none() {
                break;
            }
        }

        Ok(pool)
    }

    pub fn set_main_thread_waker(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        *self.state.waker.borrow_mut() = Some(wake);
    }

    pub fn worker_waker(&self) -> Arc<dyn Fn() + Send + Sync> {
        Arc::new(move || {
            let Ok(scope) = js_sys::global().dyn_into::<DedicatedWorkerGlobalScope>() else {
                return;
            };
            CURRENT_WORKER_WAKE.with(|slot| {
                if let Some(wake_pending) = slot.borrow().as_ref() {
                    if !wake_pending.swap(true, Ordering::SeqCst)
                        && post_worker_wake(&scope).is_err()
                    {
                        wake_pending.store(false, Ordering::SeqCst);
                    }
                } else {
                    drop(post_worker_wake(&scope));
                }
            });
        })
    }

    fn try_spawn(&self) -> Result<Option<usize>, WebError> {
        let endpoint = match &self.factory {
            EndpointFactory::Dedicated(new_worker) => {
                let worker =
                    if let Some(reused) = RECYCLED_WORKERS.with(|bin| bin.borrow_mut().pop()) {
                        reused
                    } else {
                        log::debug!("spawning new worker");
                        let worker = new_worker()?;
                        let init_msg = js_sys::Object::from_entries(&js_sys::Array::of3(
                            &js_sys::Array::of2(
                                &JsValue::from("type"),
                                &js_sys::JsString::from("wasm_init"),
                            ),
                            &js_sys::Array::of2(&JsValue::from("module"), &wasm_bindgen::module()),
                            &js_sys::Array::of2(&JsValue::from("memory"), &wasm_bindgen::memory()),
                        ))?;
                        worker.post_message(&init_msg)?;
                        worker
                    };
                WorkerEndpoint::Dedicated(worker)
            }
            EndpointFactory::Port(new_port) => match new_port()? {
                Some(port) => WorkerEndpoint::Port(port),
                None => {
                    self.state.factory_exhausted.set(true);
                    return Ok(None);
                }
            },
        };

        let state = Rc::downgrade(&self.state);
        let wake_pending = Arc::new(AtomicBool::new(false));
        let listener_wake_pending = wake_pending.clone();
        let onmessage = Closure::wrap(Box::new(move |_event: MessageEvent| {
            listener_wake_pending.store(false, Ordering::SeqCst);
            let waker = state.upgrade().and_then(|s| s.waker.borrow().clone());
            if let Some(wake) = waker {
                wake();
            }
        }) as Box<dyn FnMut(MessageEvent)>);

        Ok(self.state.push(endpoint, wake_pending, onmessage))
    }

    fn should_ramp(&self, worker_count: usize, queued: usize) -> bool {
        if queued < RAMP_BACKLOG_THRESHOLD
            || worker_count >= self.state.max_workers
            || self.state.factory_exhausted.get()
            || !low_memory_for_ramp()
        {
            return false;
        }
        if self.state.created_at.elapsed() < Duration::from_millis(RAMP_BOOT_GRACE_MS) {
            return false;
        }
        self.state
            .last_ramp_at
            .get()
            .is_none_or(|t| t.elapsed() >= Duration::from_millis(RAMP_COOLDOWN_MS))
    }

    fn dispatch_ticket(&self, worker_index: usize) -> Result<(), WebError> {
        let workers = self.state.workers.borrow();
        let Some((worker, wake_pending)) = workers.get(worker_index).cloned() else {
            drop(workers);
            self.state.queue.rollback_claim(worker_index);
            return Err(WebError::TypeError("Invalid worker index".into()));
        };
        drop(workers);

        let ticket = Work {
            queue: Arc::clone(&self.state.queue),
            worker_index,
            wake_pending,
        };
        let work_ptr = Box::into_raw(Box::new(ticket));
        let ptr_msg = JsValue::from_f64((work_ptr as usize) as f64);
        match worker.post_message(&ptr_msg) {
            Ok(()) => Ok(()),
            Err(e) => {
                unsafe {
                    drop(Box::from_raw(work_ptr));
                }
                self.state.queue.rollback_claim(worker_index);
                Err(e.into())
            }
        }
    }

    pub fn execute(
        &self,
        f: impl (FnOnce() -> PinnedFuture) + Send + 'static,
    ) -> Result<(), WebError> {
        if self.state.workers.borrow().is_empty() {
            self.try_spawn()?;
        }
        let worker_count = self.state.workers.borrow().len();
        if worker_count == 0 {
            return Err(WebError::TypeError("Worker pool has no workers".into()));
        }
        match self.state.queue.push_and_claim(Box::new(f), worker_count) {
            ClaimOutcome::Claimed(idx) => self.dispatch_ticket(idx),
            ClaimOutcome::Backlogged { queued } => {
                if self.should_ramp(worker_count, queued) {
                    if let Some(new_idx) = self.try_spawn()? {
                        self.state.last_ramp_at.set(Some(instant::Instant::now()));
                        self.state.queue.claim_worker(new_idx);
                        return self.dispatch_ticket(new_idx);
                    }
                }
                Ok(())
            }
        }
    }
}
