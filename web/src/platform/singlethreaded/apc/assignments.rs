//! Which worker serves each tracked call, with least-in-flight dispatch and cancellation routing.

use rustc_hash::FxHashMap;

/// Workers serving calls by attempt, from admission until the call finishes or is cancelled.
#[derive(Default)]
pub(crate) struct Assignments {
    calls: FxHashMap<u64, usize>,
    in_flight: Vec<usize>,
    next_turn: usize,
}

impl Assignments {
    /// Picks the worker index in `0..worker_count` with the fewest active in-flight calls,
    /// breaking ties via rotating round-robin order.
    pub(crate) fn least_loaded_worker(&mut self, worker_count: usize) -> usize {
        if worker_count == 0 {
            return 0;
        }
        if self.in_flight.len() < worker_count {
            self.in_flight.resize(worker_count, 0);
        }
        let start = self.next_turn % worker_count;
        self.next_turn = self.next_turn.wrapping_add(1);
        let mut best_idx = start;
        let mut best_load = self.in_flight.get(start).copied().unwrap_or(0);
        if best_load == 0 {
            return best_idx;
        }
        for offset in 1..worker_count {
            let idx = (start + offset) % worker_count;
            let load = self.in_flight.get(idx).copied().unwrap_or(0);
            if load < best_load {
                best_idx = idx;
                best_load = load;
                if best_load == 0 {
                    break;
                }
            }
        }
        best_idx
    }

    /// Records that the worker at `worker` received the call made for `attempt`.
    pub(crate) fn assign(&mut self, attempt: u64, worker: usize) {
        if self.in_flight.len() <= worker {
            self.in_flight.resize(worker + 1, 0);
        }
        if let Some(prev) = self.calls.insert(attempt, worker) {
            if let Some(slot) = self.in_flight.get_mut(prev) {
                *slot = slot.saturating_sub(1);
            }
        }
        if let Some(slot) = self.in_flight.get_mut(worker) {
            *slot = slot.saturating_add(1);
        }
    }

    /// Forgets a call its worker reported finished; a later cancel has nothing to stop.
    pub(crate) fn finished(&mut self, attempt: u64) {
        if let Some(worker) = self.calls.remove(&attempt) {
            if let Some(slot) = self.in_flight.get_mut(worker) {
                *slot = slot.saturating_sub(1);
            }
        }
    }

    /// The worker to tell that `attempt` is no longer wanted, if its call may still be running.
    pub(crate) fn cancel(&mut self, attempt: u64) -> Option<usize> {
        let worker = self.calls.remove(&attempt)?;
        if let Some(slot) = self.in_flight.get_mut(worker) {
            *slot = slot.saturating_sub(1);
        }
        Some(worker)
    }

    /// Drains all `(attempt, worker)` assignments still in flight when the map shuts down.
    pub(crate) fn drain(&mut self) -> impl Iterator<Item = (u64, usize)> + '_ {
        self.in_flight.fill(0);
        self.calls.drain()
    }

    /// Calls that may still be running.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.calls.len()
    }
}

#[cfg(test)]
mod tests;
