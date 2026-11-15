use std::{cell::RefCell, future::Future};

use maplibre::{benchmarking::io::scheduler::ScheduleError, io::scheduler::Scheduler};
use wasm_bindgen::{prelude::*, JsCast};
use web_sys::{MessagePort, Worker};

use super::pool::WorkerPool;
use crate::error::WebError;

const MAX_MT_WORKERS: usize = 4;

fn hardware_mt_worker_count() -> usize {
    let hc = js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("navigator"))
        .ok()
        .and_then(|nav| js_sys::Reflect::get(&nav, &JsValue::from_str("hardwareConcurrency")).ok())
        .and_then(|val| val.as_f64())
        .filter(|n| n.is_finite() && *n >= 1.0)
        .map(|n| n as usize)
        .unwrap_or(4);
    (hc / 2).clamp(2, MAX_MT_WORKERS)
}

pub struct WebWorkerPoolScheduler {
    pool: WorkerPool,
}

impl WebWorkerPoolScheduler {
    pub fn new(new_worker: js_sys::Function) -> Result<Self, WebError> {
        let count = hardware_mt_worker_count();
        let pool = WorkerPool::new(
            count,
            Box::new(move || {
                new_worker
                    .call0(&JsValue::undefined())
                    .map_err(WebError::from)?
                    .dyn_into::<Worker>()
                    .map_err(|_e| WebError::TypeError("Unable to cast to Worker".into()))
            }),
        )?;
        Ok(Self { pool })
    }

    pub fn from_ports(new_port: js_sys::Function) -> Result<Self, WebError> {
        let mut ports = Vec::with_capacity(MAX_MT_WORKERS);
        for _ in 0..MAX_MT_WORKERS {
            let val = new_port
                .call0(&JsValue::undefined())
                .map_err(WebError::from)?;
            if val.is_undefined() || val.is_null() {
                break;
            }
            let port = val
                .dyn_into::<MessagePort>()
                .map_err(|_e| WebError::TypeError("Unable to cast to MessagePort".into()))?;
            ports.push(port);
        }
        if ports.is_empty() {
            return Err(WebError::TypeError(
                "No MessagePorts supplied to WebWorkerPoolScheduler".into(),
            ));
        }
        let count = ports.len();
        let iter = RefCell::new(ports.into_iter());
        let pool = WorkerPool::from_ports(
            count,
            Box::new(move || {
                iter.borrow_mut()
                    .next()
                    .ok_or_else(|| WebError::TypeError("Exhausted pre-bound MessagePorts".into()))
            }),
        )?;
        Ok(Self { pool })
    }
}

impl Scheduler for WebWorkerPoolScheduler {
    fn schedule<T>(
        &self,
        future_factory: impl (FnOnce() -> T) + Send + 'static,
    ) -> Result<(), ScheduleError>
    where
        T: Future<Output = ()> + 'static,
    {
        self.pool
            .execute(move || Box::pin(future_factory()))
            .map_err(|e| ScheduleError::Scheduling(Box::new(e)))
    }

    fn set_main_thread_waker(&self, wake: std::sync::Arc<dyn Fn() + Send + Sync>) {
        self.pool.set_main_thread_waker(wake);
    }

    fn wrap_worker_waker(
        &self,
        _wake: std::sync::Arc<dyn Fn() + Send + Sync>,
    ) -> std::sync::Arc<dyn Fn() + Send + Sync> {
        self.pool.worker_waker()
    }
}
