use std::{
    collections::VecDeque,
    sync::{Mutex, MutexGuard},
};

pub type PinnedFuture = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'static>>;
pub(super) type Execute = Box<dyn (FnOnce() -> PinnedFuture) + Send>;

/// Concurrent async tasks claimed per worker so network `fetch` I/O overlaps while keeping
/// excess raw payloads queued instead of pinning shared WASM memory across dormant slots.
const PER_WORKER_CONCURRENCY: usize = 2;

pub(super) enum ClaimOutcome {
    Claimed(usize),
    Backlogged { queued: usize },
}

fn lock_queue<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[derive(Default)]
struct QueueInner {
    jobs: VecDeque<Execute>,
    in_flight: Vec<usize>,
    next_turn: usize,
    closed: bool,
}

pub(crate) struct SharedWorkQueue {
    inner: Mutex<QueueInner>,
}

impl SharedWorkQueue {
    pub(super) fn new() -> Self {
        Self {
            inner: Mutex::new(QueueInner::default()),
        }
    }

    /// Appends `job` to the shared queue and claims a dispatch slot on the least-loaded active
    /// worker below [`PER_WORKER_CONCURRENCY`], or reports backlog depth when saturated.
    pub(super) fn push_and_claim(&self, job: Execute, worker_count: usize) -> ClaimOutcome {
        let mut inner = lock_queue(&self.inner);
        inner.jobs.push_back(job);
        if worker_count == 0 {
            return ClaimOutcome::Backlogged {
                queued: inner.jobs.len(),
            };
        }
        if inner.in_flight.len() < worker_count {
            inner.in_flight.resize(worker_count, 0);
        }
        let start = inner.next_turn % worker_count;
        inner.next_turn = inner.next_turn.wrapping_add(1);

        let mut best: Option<(usize, usize)> = None;
        for offset in 0..worker_count {
            let idx = (start + offset) % worker_count;
            let load = inner.in_flight.get(idx).copied().unwrap_or(0);
            if load >= PER_WORKER_CONCURRENCY {
                continue;
            }
            if best.is_none_or(|(_, best_load)| load < best_load) {
                best = Some((idx, load));
                if load == 0 {
                    break;
                }
            }
        }
        if let Some((idx, _)) = best {
            if let Some(slot) = inner.in_flight.get_mut(idx) {
                *slot = slot.saturating_add(1);
            }
            ClaimOutcome::Claimed(idx)
        } else {
            ClaimOutcome::Backlogged {
                queued: inner.jobs.len(),
            }
        }
    }

    pub(super) fn claim_worker(&self, worker_index: usize) {
        let mut inner = lock_queue(&self.inner);
        if inner.in_flight.len() <= worker_index {
            inner.in_flight.resize(worker_index + 1, 0);
        }
        if let Some(slot) = inner.in_flight.get_mut(worker_index) {
            *slot = slot.saturating_add(1);
        }
    }

    pub(super) fn rollback_claim(&self, worker_index: usize) {
        let mut inner = lock_queue(&self.inner);
        if let Some(slot) = inner.in_flight.get_mut(worker_index) {
            *slot = slot.saturating_sub(1);
        }
    }

    pub(super) fn pop_or_release(&self, worker_index: usize) -> Option<Execute> {
        let mut inner = lock_queue(&self.inner);
        if inner.closed {
            if let Some(slot) = inner.in_flight.get_mut(worker_index) {
                *slot = slot.saturating_sub(1);
            }
            return None;
        }
        if let Some(job) = inner.jobs.pop_front() {
            Some(job)
        } else {
            if let Some(slot) = inner.in_flight.get_mut(worker_index) {
                *slot = slot.saturating_sub(1);
            }
            None
        }
    }

    pub(super) fn close_and_clear(&self) {
        let drained = {
            let mut inner = lock_queue(&self.inner);
            inner.closed = true;
            std::mem::take(&mut inner.jobs)
        };
        drop(drained);
    }
}
