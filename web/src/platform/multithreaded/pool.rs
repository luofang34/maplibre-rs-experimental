//! Web worker pool for shared-memory (`wasm32 + atomics`) execution with a shared work-stealing
//! queue (`Arc<SharedWorkQueue>`) and coalesced cross-realm wake notifications.
#![expect(
    unsafe_code,
    reason = "frees boxed Work ticket when posting message to worker fails"
)]

use std::{
    cell::RefCell,
    collections::VecDeque,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard,
    },
};

use wasm_bindgen::{prelude::*, JsCast};
use web_sys::{DedicatedWorkerGlobalScope, MessageEvent, MessagePort, Worker};

use crate::error::WebError;

pub type PinnedFuture = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'static>>;

type NewWorker = Box<dyn Fn() -> Result<Worker, WebError>>;
type NewPort = Box<dyn Fn() -> Result<MessagePort, WebError>>;
type Execute = Box<dyn (FnOnce() -> PinnedFuture) + Send>;

const MAX_RECYCLED_WORKERS: usize = 8;
/// Concurrent async tasks claimed per worker so network `fetch` I/O overlaps while excess jobs
/// stay in [`SharedWorkQueue`] for whichever worker finishes its CPU work first to steal.
const PER_WORKER_CONCURRENCY: usize = 2;

thread_local! {
    static RECYCLED_WORKERS: RefCell<Vec<Worker>> = const { RefCell::new(Vec::new()) };
    static CURRENT_WORKER_WAKE: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

fn lock_queue<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[derive(Default)]
struct QueueInner {
    jobs: VecDeque<Execute>,
    in_flight: Vec<usize>,
    next_turn: usize,
    closed: bool,
}

pub(crate) struct SharedWorkQueue {
    inner: Mutex<QueueInner>,
}

impl SharedWorkQueue {
    fn new() -> Self {
        Self {
            inner: Mutex::new(QueueInner::default()),
        }
    }

    /// Appends `job` to the shared queue and claims a dispatch slot on the least-loaded worker
    /// below [`PER_WORKER_CONCURRENCY`], or returns `None` when all workers are already active
    /// and will steal `job` directly in [`Self::pop_or_release`] without another `postMessage`.
    fn push_and_claim(&self, job: Execute, worker_count: usize) -> Option<usize> {
        let mut inner = lock_queue(&self.inner);
        inner.jobs.push_back(job);
        if worker_count == 0 {
            return None;
        }
        if inner.in_flight.len() < worker_count {
            inner.in_flight.resize(worker_count, 0);
        }
        let start = inner.next_turn % worker_count;
        inner.next_turn = inner.next_turn.wrapping_add(1);

        let mut best: Option<(usize, usize)> = None;
        for offset in 0..worker_count {
            let idx = (start + offset) % worker_count;
            let load = inner.in_flight.get(idx).copied().unwrap_or(0);
            if load >= PER_WORKER_CONCURRENCY {
                continue;
            }
            if best.is_none_or(|(_, best_load)| load < best_load) {
                best = Some((idx, load));
                if load == 0 {
                    break;
                }
            }
        }
        if let Some((idx, _)) = best {
            if let Some(slot) = inner.in_flight.get_mut(idx) {
                *slot = slot.saturating_add(1);
            }
            Some(idx)
        } else {
            None
        }
    }

    fn rollback_claim(&self, worker_index: usize) {
        let mut inner = lock_queue(&self.inner);
        if let Some(slot) = inner.in_flight.get_mut(worker_index) {
            *slot = slot.saturating_sub(1);
        }
    }

    fn pop_or_release(&self, worker_index: usize) -> Option<Execute> {
        let mut inner = lock_queue(&self.inner);
        if inner.closed {
            if let Some(slot) = inner.in_flight.get_mut(worker_index) {
                *slot = slot.saturating_sub(1);
            }
            return None;
        }
        if let Some(job) = inner.jobs.pop_front() {
            Some(job)
        } else {
            if let Some(slot) = inner.in_flight.get_mut(worker_index) {
                *slot = slot.saturating_sub(1);
            }
            None
        }
    }

    fn close_and_clear(&self) {
        let drained = {
            let mut inner = lock_queue(&self.inner);
            inner.closed = true;
            std::mem::take(&mut inner.jobs)
        };
        drop(drained);
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

#[cfg_attr(not(feature = "winit-window"), allow(dead_code))]
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
}

impl PoolState {
    fn push(
        &self,
        endpoint: WorkerEndpoint,
        wake_pending: Arc<AtomicBool>,
        onmessage: WorkerListener,
    ) {
        let mut workers = self.workers.borrow_mut();
        if workers
            .iter()
            .any(|(existing, _)| existing.as_js_value() == endpoint.as_js_value())
        {
            return;
        }
        endpoint.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        workers.push((endpoint, wake_pending));
        self.listeners.borrow_mut().push(onmessage);
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
        #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
        super::alloc::drain_thread_magazine();
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
    /// Creates a `WorkerPool` primed with `initial` dedicated web workers.
    #[cfg_attr(not(feature = "winit-window"), allow(dead_code))]
    pub fn new(initial: usize, new_worker: NewWorker) -> Result<WorkerPool, WebError> {
        Self::with_factory(initial, EndpointFactory::Dedicated(new_worker))
    }

    /// Creates a `WorkerPool` backed by `initial` pre-connected `MessagePort`s (already primed
    /// with `wasm_init` by the `OffscreenCanvas` render worker).
    pub fn from_ports(initial: usize, new_port: NewPort) -> Result<WorkerPool, WebError> {
        Self::with_factory(initial, EndpointFactory::Port(new_port))
    }

    fn with_factory(initial: usize, factory: EndpointFactory) -> Result<WorkerPool, WebError> {
        let pool = WorkerPool {
            factory,
            state: Rc::new(PoolState {
                workers: RefCell::new(Vec::with_capacity(initial)),
                queue: Arc::new(SharedWorkQueue::new()),
                waker: RefCell::new(None),
                listeners: RefCell::new(Vec::with_capacity(initial)),
            }),
        };
        for _ in 0..initial {
            pool.spawn()?;
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

    fn spawn(&self) -> Result<(), WebError> {
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
            EndpointFactory::Port(new_port) => WorkerEndpoint::Port(new_port()?),
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

        self.state.push(endpoint, wake_pending, onmessage);
        Ok(())
    }

    pub fn execute(
        &self,
        f: impl (FnOnce() -> PinnedFuture) + Send + 'static,
    ) -> Result<(), WebError> {
        if self.state.workers.borrow().is_empty() {
            self.spawn()?;
        }
        let workers = self.state.workers.borrow();
        if workers.is_empty() {
            return Err(WebError::TypeError("Worker pool has no workers".into()));
        }
        let Some(worker_index) = self.state.queue.push_and_claim(Box::new(f), workers.len()) else {
            return Ok(());
        };
        let Some((worker, wake_pending)) = workers.get(worker_index).cloned() else {
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
        let call_msg = js_sys::Object::from_entries(&js_sys::Array::of2(
            &js_sys::Array::of2(&JsValue::from("type"), &js_sys::JsString::from("pool_call")),
            &js_sys::Array::of2(&JsValue::from("work_ptr"), &JsValue::from(work_ptr as u32)),
        ));
        match call_msg.and_then(|msg| worker.post_message(&msg)) {
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
}
