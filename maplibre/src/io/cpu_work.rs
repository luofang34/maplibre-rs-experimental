//! CPU-bound tile work, kept off the threads that drive source fetches.
//!
//! Natively, worker procedures run as tasks on the Tokio runtime's async threads; decoding and
//! tessellating there holds a thread that other tiles' fetches are waiting to be polled on.
//! The work runs on the runtime's blocking pool instead. Browser workers have no such pool and
//! run it inline.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use thiserror::Error;

use crate::io::apc::{Context, IntoMessage, SendError};

/// CPU-bound work did not run to completion: it panicked, or the runtime shut down first.
#[derive(Error, Debug)]
#[error("CPU-bound tile work did not complete")]
pub struct CpuWorkError(#[source] Box<dyn std::error::Error + Send + Sync>);

/// Runs `work` where it does not hold up fetches, and returns what it produced.
pub(crate) async fn run_cpu_bound<R: Send + 'static>(
    work: impl FnOnce() -> R + Send + 'static,
) -> Result<R, CpuWorkError> {
    crate::platform::run_cpu_bound(work)
        .await
        .map_err(CpuWorkError)
}

/// Runs `work`, which replies through `context`, as [`run_cpu_bound`] does.
///
/// Work on the blocking pool cannot be aborted, so once the call awaiting it is dropped (the
/// map cancelled it) every further reply fails with [`SendError::Abandoned`], which ends the
/// work at its next reply instead of finishing a tile nobody wants.
pub(crate) async fn run_cpu_bound_replying<C: Context + Send, R: Send + 'static>(
    context: C,
    work: impl FnOnce(WhileWanted<C>) -> R + Send + 'static,
) -> Result<R, CpuWorkError> {
    let wanted = Wanted(Arc::new(AtomicBool::new(true)));
    let context = WhileWanted {
        context,
        wanted: wanted.0.clone(),
    };
    let result = run_cpu_bound(move || work(context)).await;
    drop(wanted);
    result
}

/// A reply context that refuses replies once the call it was handed out for is dropped.
#[derive(Clone)]
pub(crate) struct WhileWanted<C> {
    context: C,
    wanted: Arc<AtomicBool>,
}

impl<C: Context> Context for WhileWanted<C> {
    fn send_back<T: IntoMessage>(&self, message: T) -> Result<(), SendError> {
        if !self.wanted.load(Ordering::Acquire) {
            return Err(SendError::Abandoned);
        }
        self.context.send_back(message)
    }
}

/// Marks the work unwanted when the call awaiting it ends, however it ends.
struct Wanted(Arc<AtomicBool>);

impl Drop for Wanted {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
