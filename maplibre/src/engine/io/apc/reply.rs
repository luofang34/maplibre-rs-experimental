//! Cancellable CPU-bound worker helpers that reply through an APC [`Context`].

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use crate::io::{
    apc::{Context, IntoMessage, SendError},
    cpu_work::{run_cpu_bound, CpuWorkError},
};

/// Runs `work`, which replies through `context`, on the blocking pool when available.
///
/// Work on the blocking pool cannot be aborted, so once the call awaiting it is dropped (the
/// map cancelled it) every further reply fails with [`SendError::Abandoned`], which ends the
/// work at its next reply instead of finishing a tile nobody wants.
pub(crate) async fn run_cpu_bound_replying<C: Context + Send, R: Send + 'static>(
    context: C,
    work: impl FnOnce(WhileWanted<C>) -> R + Send + 'static,
) -> Result<R, CpuWorkError> {
    let wanted = Wanted(new_wanted_flag());
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
        if !is_still_wanted(&self.wanted) {
            return Err(SendError::Abandoned);
        }
        self.context.send_back(message)
    }
}

/// Marks the work unwanted when the call awaiting it ends, however it ends.
struct Wanted(Arc<AtomicBool>);

impl Drop for Wanted {
    fn drop(&mut self) {
        mark_unwanted(&self.0);
    }
}

fn new_wanted_flag() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(true))
}

fn is_still_wanted(flag: &AtomicBool) -> bool {
    flag.load(Ordering::Acquire)
}

fn mark_unwanted(flag: &AtomicBool) {
    flag.store(false, Ordering::Release);
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
