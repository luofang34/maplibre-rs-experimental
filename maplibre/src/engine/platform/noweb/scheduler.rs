//! Admission of native worker futures to the entered Tokio runtime.

use std::future::Future;

use crate::io::scheduler::{ScheduleError, Scheduler};

/// Runs detached tasks on the entered Tokio runtime when `thread-safe-futures` is enabled.
///
/// Without that feature, scheduling returns [`ScheduleError::NotImplemented`]. A caller must
/// keep the runtime alive until its accepted work completes; dropping this scheduler does
/// not cancel tasks.
pub struct TokioScheduler;

impl TokioScheduler {
    /// Creates a scheduler without creating or entering a runtime.
    pub fn new() -> Self {
        Self {}
    }
}

impl Scheduler for TokioScheduler {
    #[cfg(feature = "thread-safe-futures")]
    fn schedule<T>(
        &self,
        future_factory: impl FnOnce() -> T + Send + 'static,
    ) -> Result<(), ScheduleError>
    where
        T: Future<Output = ()> + Send + 'static,
    {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|error| ScheduleError::Scheduling(Box::new(error)))?;
        runtime.spawn(future_factory());
        Ok(())
    }

    #[cfg(not(feature = "thread-safe-futures"))]
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

impl Default for TokioScheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
