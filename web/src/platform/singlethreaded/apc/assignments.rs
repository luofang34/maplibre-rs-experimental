//! Which worker serves each tracked call, so a cancellation reaches the worker running it.

use std::collections::HashMap;

/// Workers serving calls by attempt, from admission until the call finishes or is cancelled.
#[derive(Default)]
pub(crate) struct Assignments(HashMap<u64, usize>);

impl Assignments {
    /// Records that the worker at `worker` received the call made for `attempt`.
    pub(crate) fn assign(&mut self, attempt: u64, worker: usize) {
        self.0.insert(attempt, worker);
    }

    /// Forgets a call its worker reported finished; a later cancel has nothing to stop.
    pub(crate) fn finished(&mut self, attempt: u64) {
        self.0.remove(&attempt);
    }

    /// The worker to tell that `attempt` is no longer wanted, if its call may still be running.
    pub(crate) fn cancel(&mut self, attempt: u64) -> Option<usize> {
        self.0.remove(&attempt)
    }

    /// Calls that may still be running.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }
}

#[cfg(test)]
mod tests;
