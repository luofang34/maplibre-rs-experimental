//! CPU-bound tile work, kept off the threads that drive source fetches.
//!
//! Natively, worker procedures run as tasks on the Tokio runtime's async threads; decoding and
//! tessellating there holds a thread that other tiles' fetches are waiting to be polled on.
//! The work runs on the runtime's blocking pool instead. Browser workers have no such pool and
//! run it inline.

use thiserror::Error;

/// CPU-bound work did not run to completion: it panicked, or the runtime shut down first.
#[derive(Error, Debug)]
#[error("CPU-bound tile work did not complete")]
pub struct CpuWorkError(#[source] Box<dyn std::error::Error + Send + Sync>);

/// Runs `work` where it does not hold up fetches, and returns what it produced.
pub(crate) async fn run_cpu_bound<R: Send + 'static>(
    work: impl FnOnce() -> R + Send + 'static,
) -> Result<R, CpuWorkError> {
    #[cfg(all(not(target_arch = "wasm32"), feature = "native-io"))]
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        return runtime
            .spawn_blocking(work)
            .await
            .map_err(|error| CpuWorkError(Box::new(error)));
    }
    Ok(work())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
