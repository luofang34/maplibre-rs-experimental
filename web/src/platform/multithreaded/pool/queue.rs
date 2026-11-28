use std::{
    collections::VecDeque,
    sync::{Mutex, MutexGuard},
};

pub type PinnedFuture = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'static>>;
pub(super) type Execute = Box<dyn (FnOnce() -> PinnedFuture) + Send>;

/// Concurrent async tasks claimed per worker so network `fetch` I/O overlaps while keeping
/// excess raw payloads bounded by `limiter`.
const PER_WORKER_CONCURRENCY: usize = 4;

pub(super) enum ClaimOutcome {
    Claimed {
        worker_index: usize,
        total_in_flight: usize,
    },
    Backlogged {
        queued: usize,
    },
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
    pending_tickets: Vec<usize>,
    ready: Vec<bool>,
    next_turn: usize,
    closed: bool,
}

fn record_claim(inner: &mut QueueInner, worker_index: usize) {
    if inner.in_flight.len() <= worker_index {
        inner.in_flight.resize(worker_index + 1, 0);
    }
    if inner.pending_tickets.len() <= worker_index {
        inner.pending_tickets.resize(worker_index + 1, 0);
    }
    if let Some(slot) = inner.in_flight.get_mut(worker_index) {
        *slot = slot.saturating_add(1);
    }
    if let Some(pending) = inner.pending_tickets.get_mut(worker_index) {
        *pending = pending.saturating_add(1);
    }
}

pub(crate) struct SharedWorkQueue {
    inner: Mutex<QueueInner>,
}

fn pick_worker(
    inner: &QueueInner,
    worker_count: usize,
    start: usize,
    require_ready: bool,
) -> Option<usize> {
    let mut best: Option<(usize, usize)> = None;
    for offset in 0..worker_count {
        let idx = (start + offset) % worker_count;
        if require_ready && !inner.ready.get(idx).copied().unwrap_or(false) {
            continue;
        }
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
    best.map(|(idx, _)| idx)
}

/// Least-loaded slot starting at `start` before any worker reports ready so initial boot bursts
/// fan out evenly across all eager workers instead of saturating slot 0 first.
fn first_slot_with_capacity(
    inner: &QueueInner,
    worker_count: usize,
    start: usize,
) -> Option<usize> {
    pick_worker(inner, worker_count, start, false)
}

impl SharedWorkQueue {
    pub(super) fn new() -> Self {
        Self {
            inner: Mutex::new(QueueInner::default()),
        }
    }

    pub(super) fn mark_ready(&self, worker_index: usize) {
        let mut inner = lock_queue(&self.inner);
        if inner.ready.len() <= worker_index {
            inner.ready.resize(worker_index + 1, false);
        }
        if let Some(flag) = inner.ready.get_mut(worker_index) {
            *flag = true;
        }
    }

    pub(super) fn claim_backlogged_for(&self, worker_index: usize) -> bool {
        let mut inner = lock_queue(&self.inner);
        if inner.closed {
            return false;
        }
        let ready_pending_tickets: usize = inner
            .pending_tickets
            .iter()
            .enumerate()
            .filter(|&(i, _)| inner.ready.get(i).copied().unwrap_or(false))
            .map(|(_, &pending)| pending)
            .sum();
        if inner.jobs.len() <= ready_pending_tickets {
            return false;
        }
        if inner.in_flight.get(worker_index).copied().unwrap_or(0) >= PER_WORKER_CONCURRENCY {
            return false;
        }
        record_claim(&mut inner, worker_index);
        true
    }

    /// Appends `job` to the shared queue and claims a dispatch slot on the least-loaded ready
    /// worker below [`PER_WORKER_CONCURRENCY`] (falling back to slot 0 / unready workers when no
    /// worker has finished WASM initialization yet), or reports backlog depth when saturated.
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
        if inner.pending_tickets.len() < worker_count {
            inner.pending_tickets.resize(worker_count, 0);
        }
        if inner.ready.len() < worker_count {
            inner.ready.resize(worker_count, false);
        }
        let any_ready = inner
            .ready
            .get(..worker_count)
            .is_some_and(|ready| ready.iter().any(|&r| r));
        let start = {
            let s = inner.next_turn % worker_count;
            inner.next_turn = inner.next_turn.wrapping_add(1);
            s
        };
        let chosen = if any_ready {
            pick_worker(&inner, worker_count, start, true)
                .or_else(|| pick_worker(&inner, worker_count, start, false))
        } else {
            first_slot_with_capacity(&inner, worker_count, start)
        };
        if let Some(idx) = chosen {
            record_claim(&mut inner, idx);
            let total_in_flight = inner.in_flight.iter().sum();
            ClaimOutcome::Claimed {
                worker_index: idx,
                total_in_flight,
            }
        } else {
            ClaimOutcome::Backlogged {
                queued: inner.jobs.len(),
            }
        }
    }

    pub(super) fn claim_worker(&self, worker_index: usize) {
        let mut inner = lock_queue(&self.inner);
        record_claim(&mut inner, worker_index);
    }

    pub(super) fn rollback_claim(&self, worker_index: usize) {
        let mut inner = lock_queue(&self.inner);
        if let Some(slot) = inner.in_flight.get_mut(worker_index) {
            *slot = slot.saturating_sub(1);
        }
        if let Some(pending) = inner.pending_tickets.get_mut(worker_index) {
            *pending = pending.saturating_sub(1);
        }
    }

    pub(super) fn pop_or_release(
        &self,
        worker_index: usize,
        ticket_started: &mut bool,
    ) -> Option<Execute> {
        let mut inner = lock_queue(&self.inner);
        if !*ticket_started {
            *ticket_started = true;
            if let Some(pending) = inner.pending_tickets.get_mut(worker_index) {
                *pending = pending.saturating_sub(1);
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn noop_job() -> Execute {
        Box::new(|| Box::pin(async {}))
    }

    #[test]
    fn claims_up_to_per_worker_concurrency_then_backlogs() {
        let queue = SharedWorkQueue::new();
        for expected in 1..=(2 * PER_WORKER_CONCURRENCY) {
            let outcome = queue.push_and_claim(noop_job(), 2);
            assert!(matches!(
                outcome,
                ClaimOutcome::Claimed { total_in_flight, .. } if total_in_flight == expected
            ));
        }
        let overflow = queue.push_and_claim(noop_job(), 2);
        assert!(matches!(
            overflow,
            ClaimOutcome::Backlogged { queued } if queued == 9
        ));
        let mut started = false;
        assert!(queue.pop_or_release(0, &mut started).is_some());
        assert!(started);
        queue.close_and_clear();
        assert!(queue.pop_or_release(0, &mut started).is_none());
    }

    #[test]
    fn assigns_initial_burst_across_slots_when_none_ready() {
        let queue = SharedWorkQueue::new();
        for expected in 0..3 {
            assert!(matches!(
                queue.push_and_claim(noop_job(), 3),
                ClaimOutcome::Claimed { worker_index, .. } if worker_index == expected
            ));
        }
        assert!(matches!(
            queue.push_and_claim(noop_job(), 3),
            ClaimOutcome::Claimed {
                worker_index: 0,
                ..
            }
        ));
    }

    #[test]
    fn prefers_ready_workers_over_unready_workers() {
        let queue = SharedWorkQueue::new();
        queue.mark_ready(1);
        for _ in 0..PER_WORKER_CONCURRENCY {
            assert!(matches!(
                queue.push_and_claim(noop_job(), 2),
                ClaimOutcome::Claimed {
                    worker_index: 1,
                    ..
                }
            ));
        }
        assert!(matches!(
            queue.push_and_claim(noop_job(), 2),
            ClaimOutcome::Claimed {
                worker_index: 0,
                ..
            }
        ));
    }

    #[test]
    fn mark_ready_drains_backlogged_jobs_via_claim_backlogged_for() {
        let queue = SharedWorkQueue::new();
        for _ in 0..PER_WORKER_CONCURRENCY {
            assert!(matches!(
                queue.push_and_claim(noop_job(), 1),
                ClaimOutcome::Claimed {
                    worker_index: 0,
                    ..
                }
            ));
        }
        assert!(matches!(
            queue.push_and_claim(noop_job(), 1),
            ClaimOutcome::Backlogged { queued: 5 }
        ));
        assert!(matches!(
            queue.push_and_claim(noop_job(), 1),
            ClaimOutcome::Backlogged { queued: 6 }
        ));

        queue.mark_ready(1);
        assert!(queue.claim_backlogged_for(1));
        assert!(queue.claim_backlogged_for(1));
        for _ in 0..(PER_WORKER_CONCURRENCY - 2) {
            assert!(queue.claim_backlogged_for(1));
        }
        assert!(!queue.claim_backlogged_for(1));
    }

    #[test]
    fn claim_backlogged_for_does_not_overclaim_single_queued_job() {
        let queue = SharedWorkQueue::new();
        assert!(matches!(
            queue.push_and_claim(noop_job(), 2),
            ClaimOutcome::Claimed {
                worker_index: 0,
                ..
            }
        ));
        queue.mark_ready(1);
        assert!(queue.claim_backlogged_for(1));
        assert!(!queue.claim_backlogged_for(1));
    }

    #[test]
    fn claim_backlogged_for_does_not_starve_behind_running_ready_worker() {
        let queue = SharedWorkQueue::new();
        queue.mark_ready(0);
        for _ in 0..PER_WORKER_CONCURRENCY {
            assert!(matches!(
                queue.push_and_claim(noop_job(), 1),
                ClaimOutcome::Claimed {
                    worker_index: 0,
                    ..
                }
            ));
        }
        // Simulate all 4 tasks on Worker 0 having popped their jobs and awaiting network I/O.
        for _ in 0..PER_WORKER_CONCURRENCY {
            let mut started = false;
            assert!(queue.pop_or_release(0, &mut started).is_some());
            assert!(started);
        }
        // 4 more jobs arrive and remain backlogged while Worker 0 is busy.
        for expected in 1..=PER_WORKER_CONCURRENCY {
            assert!(matches!(
                queue.push_and_claim(noop_job(), 1),
                ClaimOutcome::Backlogged { queued } if queued == expected
            ));
        }

        queue.mark_ready(1);
        for _ in 0..PER_WORKER_CONCURRENCY {
            assert!(queue.claim_backlogged_for(1));
        }
        assert!(!queue.claim_backlogged_for(1));
    }

    #[test]
    fn claim_backlogged_for_respects_ready_unpopped_tickets_without_overclaiming() {
        let queue = SharedWorkQueue::new();
        queue.mark_ready(0);
        assert!(matches!(
            queue.push_and_claim(noop_job(), 2),
            ClaimOutcome::Claimed {
                worker_index: 0,
                ..
            }
        ));
        queue.mark_ready(1);
        assert!(!queue.claim_backlogged_for(1));
    }

    #[test]
    fn rollback_and_empty_first_pop_release_pending_tickets() {
        let queue = SharedWorkQueue::new();
        queue.mark_ready(0);
        assert!(matches!(
            queue.push_and_claim(noop_job(), 2),
            ClaimOutcome::Claimed {
                worker_index: 0,
                ..
            }
        ));
        queue.rollback_claim(0);
        queue.mark_ready(1);
        assert!(queue.claim_backlogged_for(1));
        assert!(!queue.claim_backlogged_for(1));

        // Drain the single job via worker 0, then worker 1's ticket pops empty and releases its
        // pending ticket so future backlog isn't suppressed.
        let mut started0 = false;
        assert!(queue.pop_or_release(0, &mut started0).is_some());
        let mut started1 = false;
        assert!(queue.pop_or_release(1, &mut started1).is_none());
        assert!(started1);

        assert!(matches!(
            queue.push_and_claim(noop_job(), 0),
            ClaimOutcome::Backlogged { queued: 1 }
        ));
        assert!(queue.claim_backlogged_for(0));
    }
}
