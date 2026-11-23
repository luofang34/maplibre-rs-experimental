//! The calls a worker runs, so the main thread can stop one it no longer wants.

use futures::future::{AbortHandle, AbortRegistration};
use rustc_hash::FxHashMap;

/// Abort handles of the calls this worker runs, by attempt.
#[derive(Default)]
pub(crate) struct RunningCalls(FxHashMap<u64, AbortHandle>);

impl RunningCalls {
    /// Registers the call for `attempt`; the call stops at its next await once cancelled.
    pub(crate) fn start(&mut self, attempt: u64) -> AbortRegistration {
        let (handle, registration) = AbortHandle::new_pair();
        self.0.insert(attempt, handle);
        registration
    }

    /// Forgets the call for `attempt`, which ended either way.
    pub(crate) fn finish(&mut self, attempt: u64) {
        self.0.remove(&attempt);
    }

    /// Stops the call for `attempt`; `false` when it is not running.
    pub(crate) fn cancel(&mut self, attempt: u64) -> bool {
        let Some(handle) = self.0.remove(&attempt) else {
            return false;
        };
        handle.abort();
        true
    }

    /// Stops every active call listed in `attempts`.
    pub(crate) fn cancel_batch(&mut self, attempts: &[u64]) {
        for &attempt in attempts {
            if let Some(handle) = self.0.remove(&attempt) {
                handle.abort();
            }
        }
    }

    /// Whether no active worker calls are tracked.
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests;
