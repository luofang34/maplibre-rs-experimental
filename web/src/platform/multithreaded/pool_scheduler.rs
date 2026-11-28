use std::future::Future;

use maplibre::{benchmarking::io::scheduler::ScheduleError, io::scheduler::Scheduler};
use wasm_bindgen::{prelude::*, JsCast};
use web_sys::MessagePort;
#[cfg(feature = "winit-window")]
use web_sys::Worker;

use super::pool::WorkerPool;
use crate::error::WebError;

const MAX_MT_WORKERS: usize = 4;
const MAX_EAGER_MT_WORKERS: usize = 4;

fn hardware_mt_worker_count() -> usize {
    let cores = js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("navigator"))
        .ok()
        .and_then(|nav| js_sys::Reflect::get(&nav, &JsValue::from_str("hardwareConcurrency")).ok())
        .and_then(|val| val.as_f64())
        .map_or(4, |n| {
            if n.is_finite() && n >= 1.0 {
                n as usize
            } else {
                4
            }
        });
    cores.saturating_sub(1).clamp(1, MAX_MT_WORKERS)
}

const fn initial_mt_worker_count(target: usize) -> usize {
    if target == 0 {
        1
    } else if target < MAX_EAGER_MT_WORKERS {
        target
    } else {
        MAX_EAGER_MT_WORKERS
    }
}

pub struct WebWorkerPoolScheduler {
    pool: WorkerPool,
}

impl WebWorkerPoolScheduler {
    #[cfg_attr(
        not(feature = "winit-window"),
        expect(
            dead_code,
            reason = "Dedicated worker pool scheduler is only used under winit-window"
        )
    )]
    pub fn new(new_worker: js_sys::Function) -> Result<Self, WebError> {
        let max_workers = hardware_mt_worker_count();
        let initial = initial_mt_worker_count(max_workers);
        #[cfg(not(feature = "winit-window"))]
        let factory = {
            let _ = new_worker;
            Box::new(|| {
                Err(WebError::TypeError(
                    "Dedicated worker factory requires winit-window".into(),
                ))
            })
        };
        #[cfg(feature = "winit-window")]
        let factory = Box::new(move || {
            new_worker
                .call0(&JsValue::undefined())
                .map_err(WebError::from)?
                .dyn_into::<Worker>()
                .map_err(|_e| WebError::TypeError("Unable to cast to Worker".into()))
        });
        let pool = WorkerPool::new(initial, max_workers, factory)?;
        Ok(Self { pool })
    }

    pub fn from_ports(new_port: js_sys::Function) -> Result<Self, WebError> {
        let max_workers = hardware_mt_worker_count();
        let initial = initial_mt_worker_count(max_workers);
        let pool = WorkerPool::from_ports(
            initial,
            max_workers,
            Box::new(move || {
                let val = new_port
                    .call0(&JsValue::undefined())
                    .map_err(WebError::from)?;
                if val.is_undefined() || val.is_null() {
                    return Ok(None);
                }
                val.dyn_into::<MessagePort>()
                    .map(Some)
                    .map_err(|_e| WebError::TypeError("Unable to cast to MessagePort".into()))
            }),
        )?;
        if pool.worker_count() == 0 {
            return Err(WebError::TypeError(
                "No MessagePorts supplied to WebWorkerPoolScheduler".into(),
            ));
        }
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
