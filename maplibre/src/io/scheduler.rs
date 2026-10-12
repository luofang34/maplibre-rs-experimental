//! Admission of asynchronous work to a platform executor.

use std::future::Future;

use thiserror::Error;

/// A task could not be accepted by the executor.
#[derive(Error, Debug)]
pub enum ScheduleError {
    /// The executor rejected work with an underlying platform cause.
    #[error("scheduling work failed")]
    Scheduling(#[source] Box<dyn std::error::Error>),
    /// The selected scheduler cannot run futures in this platform or feature configuration.
    #[error("scheduler is not implemented for this configuration")]
    NotImplemented,
    /// A worker-backed scheduler has no worker to receive the task.
    #[error("the worker pool is empty")]
    NoWorkers,
}

/// Accepts a future factory for execution outside the caller's render loop.
///
/// The factory can cross threads; its returned future only needs `Send` when
/// `thread-safe-futures` is enabled. Success means admission, not task completion.
pub trait Scheduler: 'static {
    /// Submits a factory; rejection returns the executor cause without running its future.
    #[cfg(feature = "thread-safe-futures")]
    fn schedule<T>(
        &self,
        future_factory: impl (FnOnce() -> T) + Send + 'static,
    ) -> Result<(), ScheduleError>
    where
        T: Future<Output = ()> + Send + 'static;

    /// Submits a factory; rejection returns the executor cause without running its future.
    #[cfg(not(feature = "thread-safe-futures"))]
    fn schedule<T>(
        &self,
        future_factory: impl (FnOnce() -> T) + Send + 'static,
    ) -> Result<(), ScheduleError>
    where
        T: Future<Output = ()> + 'static;
}

/// Rejects every task without invoking its factory.
pub struct NopScheduler;

impl Scheduler for NopScheduler {
    fn schedule<T>(
        &self,
        _future_factory: impl FnOnce() -> T + Send + 'static,
    ) -> Result<(), ScheduleError>
    where
        T: Future<Output = ()> + 'static,
    {
        Err(ScheduleError::NotImplemented)
    }
}

/// Runs each task on the calling thread's browser event loop, between the host's frames.
///
/// The task is not `Send`, so it can hold `fetch` promises and other JavaScript values.
#[cfg(target_arch = "wasm32")]
pub struct LocalScheduler;

#[cfg(target_arch = "wasm32")]
impl Scheduler for LocalScheduler {
    fn schedule<T>(
        &self,
        future_factory: impl FnOnce() -> T + Send + 'static,
    ) -> Result<(), ScheduleError>
    where
        T: Future<Output = ()> + 'static,
    {
        wasm_bindgen_futures::spawn_local(future_factory());
        Ok(())
    }
}
