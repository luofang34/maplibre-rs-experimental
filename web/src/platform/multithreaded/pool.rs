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
use web_sys::{DedicatedWorkerGlobalScope, MessageEvent, Worker};

use crate::error::WebError;

pub type PinnedFuture = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'static>>;

type NewWorker = Box<dyn Fn() -> Result<Worker, WebError>>;
type Execute = Box<dyn (FnOnce() -> PinnedFuture) + Send>;

const MAX_RECYCLED_WORKERS: usize = 8;

thread_local! {
    static RECYCLED_WORKERS: RefCell<Vec<Worker>> = const { RefCell::new(Vec::new()) };
    static CURRENT_WORKER_WAKE: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

pub struct WorkerPool {
    new_worker: NewWorker,
    state: Rc<PoolState>,
}

type WorkerListener = Closure<dyn FnMut(MessageEvent)>;

struct PoolState {
    workers: RefCell<Vec<(Worker, Arc<AtomicBool>)>>,
    next_worker: Cell<usize>,
    waker: RefCell<Option<Arc<dyn Fn() + Send + Sync>>>,
    listeners: RefCell<Vec<WorkerListener>>,
}

impl PoolState {
    fn push(&self, worker: Worker, wake_pending: Arc<AtomicBool>, onmessage: WorkerListener) {
        let mut workers = self.workers.borrow_mut();
        for (existing_worker, _) in workers.iter() {
            assert_ne!(existing_worker as &JsValue, &worker as &JsValue);
        }
        worker.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        workers.push((worker, wake_pending));
        self.listeners.borrow_mut().push(onmessage);
    }
}

impl Drop for PoolState {
    fn drop(&mut self) {
        let drained = std::mem::take(self.workers.get_mut());
        self.listeners.get_mut().clear();
        RECYCLED_WORKERS.with(|bin| {
            let mut bin = bin.borrow_mut();
            for (worker, _) in drained {
                worker.set_onmessage(None);
                if bin.len() < MAX_RECYCLED_WORKERS {
                    bin.push(worker);
                } else {
                    worker.terminate();
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

impl WorkerPool {
    /// Creates a `WorkerPool` primed with `initial` dedicated web workers.
    pub fn new(initial: usize, new_worker: NewWorker) -> Result<WorkerPool, WebError> {
        let pool = WorkerPool {
            new_worker,
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
                        && scope.post_message(&JsValue::TRUE).is_err()
                    {
                        wake_pending.store(false, Ordering::Release);
                    }
                } else {
                    let _ = scope.post_message(&JsValue::TRUE);
                }
            });
        })
    }

    fn spawn(&self) -> Result<(), WebError> {
        let worker = if let Some(reused) = RECYCLED_WORKERS.with(|bin| bin.borrow_mut().pop()) {
            reused
        } else {
            log::debug!("spawning new worker");
            let worker = (self.new_worker)()?;
            let init_msg = js_sys::Object::from_entries(&js_sys::Array::of3(
                &js_sys::Array::of2(&JsValue::from("type"), &js_sys::JsString::from("wasm_init")),
                &js_sys::Array::of2(&JsValue::from("module"), &wasm_bindgen::module()),
                &js_sys::Array::of2(&JsValue::from("memory"), &wasm_bindgen::memory()),
            ))?;
            worker.post_message(&init_msg)?;
            worker
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

        self.state.push(worker, wake_pending, onmessage);
        Ok(())
    }

    fn worker(&self) -> Result<(Worker, Arc<AtomicBool>), WebError> {
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
