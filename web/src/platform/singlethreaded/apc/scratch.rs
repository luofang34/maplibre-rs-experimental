//! Reusable per-frame scratch buffers and batch admission budget for single-threaded APC.

#![deny(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use rustc_hash::{FxHashMap, FxHashSet};

use super::raw_queue::QueuedMessage;

const BOOT_MAX_DRAIN_ATTEMPTS: usize = 4;

#[derive(Default)]
pub(super) struct ReceiveScratch {
    pub(super) budget: BatchBudget,
    pub(super) attempt_totals: FxHashMap<u64, usize>,
    pub(super) deferred: Vec<QueuedMessage>,
    pub(super) cancel_buckets: Vec<Vec<u64>>,
}

impl ReceiveScratch {
    pub(super) fn admit_boot_queued<F: FnMut(&maplibre::io::apc::Message) -> bool>(
        &mut self,
        received_queued: &mut Vec<QueuedMessage>,
        buffer: &mut Vec<QueuedMessage>,
        ret: &mut Vec<QueuedMessage>,
        filter: &mut F,
    ) -> bool {
        self.budget.clear_attempts();
        self.deferred.clear();
        for item in received_queued.drain(..) {
            if let Some(id) = item.attempt() {
                if !self.budget.attempts.contains(&id)
                    && self.budget.attempts.len() >= BOOT_MAX_DRAIN_ATTEMPTS
                {
                    self.deferred.push(item);
                    continue;
                }
                self.budget.attempts.insert(id);
            }
            if item.matches(filter) {
                ret.push(item);
            } else {
                buffer.push(item);
            }
        }
        std::mem::swap(received_queued, &mut self.deferred);
        !received_queued.is_empty()
    }
}

#[derive(Default)]
pub(super) struct BatchBudget {
    bytes: usize,
    attempts: FxHashSet<u64>,
    msgs: usize,
}

impl BatchBudget {
    const MAX_RAW_BYTES: usize = 768 * 1024;
    const MAX_RAW_MESSAGES: usize = 16;
    const MAX_RAW_ATTEMPTS: usize = 3;

    pub(super) fn clear_attempts(&mut self) {
        self.attempts.clear();
    }

    pub(super) fn reset_from_buffer(&mut self, buffer: &[QueuedMessage]) {
        self.bytes = buffer.iter().map(QueuedMessage::raw_byte_len).sum();
        self.attempts.clear();
        for item in buffer {
            if let Some(id) = item.attempt() {
                self.attempts.insert(id);
            }
        }
        self.msgs = buffer.len();
    }

    pub(super) fn try_admit(
        &mut self,
        item: &QueuedMessage,
        attempt_totals: &FxHashMap<u64, usize>,
    ) -> bool {
        let attempt = item.attempt();
        let admit = match attempt {
            None => {
                self.attempts.is_empty()
                    || (self.msgs < Self::MAX_RAW_MESSAGES && self.bytes < Self::MAX_RAW_BYTES)
            }
            Some(id) => {
                self.attempts.contains(&id)
                    || self.attempts.is_empty()
                    || (self.attempts.len() < Self::MAX_RAW_ATTEMPTS
                        && self.msgs < Self::MAX_RAW_MESSAGES
                        && self
                            .bytes
                            .saturating_add(attempt_totals.get(&id).copied().unwrap_or(0))
                            <= Self::MAX_RAW_BYTES)
            }
        };
        if admit {
            if let Some(id) = attempt {
                self.attempts.insert(id);
            }
            self.msgs = self.msgs.saturating_add(1);
            self.bytes = self.bytes.saturating_add(item.raw_byte_len());
        }
        admit
    }
}
