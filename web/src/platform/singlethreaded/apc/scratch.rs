//! Reusable per-frame scratch buffers and batch admission budget for single-threaded APC.

#![deny(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use rustc_hash::{FxHashMap, FxHashSet};

use super::raw_queue::QueuedMessage;

#[derive(Default)]
pub(super) struct ReceiveScratch {
    pub(super) budget: BatchBudget,
    pub(super) attempt_totals: FxHashMap<u64, usize>,
    pub(super) deferred: Vec<QueuedMessage>,
    pub(super) cancel_buckets: Vec<Vec<u64>>,
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
