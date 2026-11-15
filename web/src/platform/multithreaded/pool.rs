//! Web worker pool for shared-memory (`wasm32 + atomics`) execution with deterministic
//! round-robin dispatch and per-worker coalesced cross-realm wake notifications.
#![allow(unsafe_code)]

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
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

thread_local! {
    static RECYCLED_WORKERS: RefCell<Vec<Worker>> = const { RefCell::new(Vec::new()) };
    static CURRENT_WORKER_WAKE: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
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
    next_worker: Cell<usize>,
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
        for (existing, _) in workers.iter() {
            assert_ne!(existing.as_js_value(), endpoint.as_js_value());
        }
        endpoint.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        workers.push((endpoint, wake_pending));
        self.listeners.borrow_mut().push(onmessage);
    }
}

impl Drop for PoolState {
    fn drop(&mut self) {
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
    func: Execute,
    wake_pending: Arc<AtomicBool>,
}

impl Work {
    pub fn execute(self) -> PinnedFuture {
        CURRENT_WORKER_WAKE.with(|slot| {
            *slot.borrow_mut() = Some(self.wake_pending);
        });
        (self.func)()
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
                next_worker: Cell::new(0),
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
                    if !wake_pending.swap(true, Ordering::AcqRel)
                        && post_worker_wake(&scope).is_err()
                    {
                        wake_pending.store(false, Ordering::Release);
                    }
                } else {
                    let _ = post_worker_wake(&scope);
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
            listener_wake_pending.store(false, Ordering::Release);
            let waker = state.upgrade().and_then(|s| s.waker.borrow().clone());
            if let Some(wake) = waker {
                wake();
            }
        }) as Box<dyn FnMut(MessageEvent)>);

        self.state.push(endpoint, wake_pending, onmessage);
        Ok(())
    }

    fn worker(&self) -> Result<(WorkerEndpoint, Arc<AtomicBool>), WebError> {
        if self.state.workers.borrow().is_empty() {
            self.spawn()?;
        }
        let workers = self.state.workers.borrow();
        if workers.is_empty() {
            return Err(WebError::TypeError("Worker pool has no workers".into()));
        }
        let turn = self.state.next_worker.get();
        self.state.next_worker.set(turn.wrapping_add(1));
        Ok(workers[turn % workers.len()].clone())
    }

    pub fn execute(
        &self,
        f: impl (FnOnce() -> PinnedFuture) + Send + 'static,
    ) -> Result<(), WebError> {
        let (worker, wake_pending) = self.worker()?;
        let work = Work {
            func: Box::new(f),
            wake_pending,
        };
        let work_ptr = Box::into_raw(Box::new(work));
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
                Err(e.into())
            }
        }
    }
}
