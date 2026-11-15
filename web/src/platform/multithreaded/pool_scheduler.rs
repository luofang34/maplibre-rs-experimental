use std::future::Future;

use maplibre::{benchmarking::io::scheduler::ScheduleError, io::scheduler::Scheduler};
use wasm_bindgen::prelude::*;
use web_sys::Worker;

use super::pool::WorkerPool;
use crate::error::WebError;

pub struct WebWorkerPoolScheduler {
    pool: WorkerPool,
}

impl WebWorkerPoolScheduler {
    pub fn new(new_worker: js_sys::Function) -> Result<Self, WebError> {
        // Two shared-memory workers overlap network/DEM fetches and symbol layout while
        // `limiter.rs` serializes heavy MVT expansion to avoid `dlmalloc` spinlock contention.
        let pool = WorkerPool::new(
            2,
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
