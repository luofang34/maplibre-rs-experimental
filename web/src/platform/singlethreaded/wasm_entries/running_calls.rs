//! The calls a worker runs, so the main thread can stop one it no longer wants.

use std::collections::HashMap;

use futures::future::{AbortHandle, AbortRegistration};

/// Abort handles of the calls this worker runs, by attempt.
#[derive(Default)]
pub(crate) struct RunningCalls(HashMap<u64, AbortHandle>);

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
}

#[cfg(test)]
mod tests;
