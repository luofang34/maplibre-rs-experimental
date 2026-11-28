//! Per-tile per-kind retry state machine and attempt component.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::RequestKind;
use crate::tcs::tiles::TileComponent;

/// Disposition after all sources in one tile request have completed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestDisposition {
    /// Every source completed without a transient failure.
    Complete,
    /// At least one source failed temporarily and can be requested again.
    Retry,
}

#[derive(Default)]
pub(super) struct RetryState {
    attempt: Option<u64>,
    pending: bool,
    delay: Duration,
    deadline: Option<Duration>,
    /// The style changed while this request was in flight, so its result is refetched.
    stale: bool,
    /// The sources the attempt was limited to, so that a retry of it is for them again;
    /// `None` for an attempt that processed every source.
    issued_for: Option<Vec<String>>,
    /// The family left the tile out of what it wanted in its last frame, so a passed retry
    /// deadline waits for the tile to come back into view instead of asking for frames.
    out_of_view: bool,
}

impl RetryState {
    #[inline]
    pub(super) fn is_pending(&self) -> bool {
        self.pending
    }

    #[inline]
    pub(super) fn matches_attempt(&self, attempt: Option<u64>) -> bool {
        self.attempt == attempt
    }

    #[cfg(all(test, feature = "headless", not(target_arch = "wasm32")))]
    #[inline]
    pub(super) fn pending_attempt(&self) -> Option<u64> {
        self.pending.then_some(self.attempt).flatten()
    }

    /// Whether the next frame has a retry of this tile to issue.
    pub(super) fn retry_due(&self, now: Duration) -> bool {
        !self.out_of_view && self.deadline.is_some_and(|due| due <= now)
    }

    #[inline]
    pub(super) fn is_due(&self, now: Duration) -> bool {
        !self.pending && self.deadline.is_some_and(|deadline| now >= deadline)
    }

    #[inline]
    pub(super) fn is_waiting(&self) -> bool {
        self.pending || self.deadline.is_some()
    }

    #[inline]
    pub(super) fn has_deadline(&self) -> bool {
        self.deadline.is_some()
    }

    #[inline]
    pub(super) fn visible_deadline(&self) -> Option<Duration> {
        (!self.out_of_view).then_some(self.deadline).flatten()
    }

    #[inline]
    pub(super) fn is_backing_off(&self, now: Duration) -> bool {
        !self.pending && self.deadline.is_some_and(|due| due > now)
    }

    pub(super) fn sync_view_presence(&mut self, in_view: bool) -> bool {
        if !self.is_waiting() {
            return false;
        }
        self.out_of_view = !in_view;
        true
    }

    /// Schedules the next attempt after a delay that doubles with each failure.
    fn back_off(&mut self, now: Duration) {
        self.delay = if self.delay.is_zero() {
            Duration::from_secs(1)
        } else {
            self.delay.saturating_mul(2).min(Duration::from_secs(30))
        };
        self.deadline = Some(now.saturating_add(self.delay));
    }

    pub(super) fn schedule_after(
        &mut self,
        now: Duration,
        delay: Duration,
        out_of_view: bool,
    ) -> bool {
        if self.is_waiting() {
            return false;
        }
        self.deadline = Some(now.saturating_add(delay));
        self.out_of_view = out_of_view;
        true
    }

    pub(super) fn mark_started(
        &mut self,
        attempt: u64,
        out_of_view: bool,
        only_sources: Option<&[String]>,
    ) {
        self.attempt = Some(attempt);
        self.pending = true;
        self.deadline = None;
        self.stale = false;
        self.out_of_view = out_of_view;
        self.issued_for = only_sources.map(<[String]>::to_vec);
    }

    pub(super) fn schedule_refresh(&mut self, now: Duration, out_of_view: bool) -> bool {
        if self.attempt.is_none() {
            return false;
        }
        if self.pending {
            self.stale = true;
            false
        } else if self.deadline.is_none() {
            // A tile already backing off retries at its deadline with the new data.
            self.delay = Duration::ZERO;
            self.deadline = Some(now);
            self.out_of_view = out_of_view;
            true
        } else {
            false
        }
    }

    pub(super) fn begin_completion(&mut self, attempt: Option<u64>) -> Option<bool> {
        if !self.pending || self.attempt != attempt {
            return None;
        }
        self.pending = false;
        Some(std::mem::take(&mut self.stale))
    }

    pub(super) fn apply_completion(
        &mut self,
        disposition: RequestDisposition,
        stale: bool,
        now: Duration,
    ) -> Option<Option<Vec<String>>> {
        match disposition {
            RequestDisposition::Complete => {
                self.delay = Duration::ZERO;
                self.deadline = stale.then_some(now);
                None
            }
            RequestDisposition::Retry => {
                self.back_off(now);
                Some(self.issued_for.clone())
            }
        }
    }

    pub(super) fn mark_rejected(&mut self, now: Duration, out_of_view: bool) {
        self.pending = false;
        self.stale = false;
        self.out_of_view = out_of_view;
        self.back_off(now);
    }

    pub(super) fn cancel_if_pending(&mut self) -> Option<Option<u64>> {
        if !self.pending {
            return None;
        }
        let attempt = self.attempt;
        *self = Self::default();
        Some(attempt)
    }
}

#[derive(Default)]
pub(crate) struct TileRequestRetries([RetryState; 3]);
impl TileComponent for TileRequestRetries {}

impl TileRequestRetries {
    pub(super) fn slot(&self, kind: RequestKind) -> &RetryState {
        let [raster, dem, vector] = &self.0;
        match kind {
            RequestKind::Raster => raster,
            RequestKind::Dem => dem,
            RequestKind::Vector => vector,
        }
    }

    pub(super) fn slot_mut(&mut self, kind: RequestKind) -> &mut RetryState {
        let [raster, dem, vector] = &mut self.0;
        match kind {
            RequestKind::Raster => raster,
            RequestKind::Dem => dem,
            RequestKind::Vector => vector,
        }
    }

    pub(super) fn slots_mut(&mut self) -> &mut [RetryState; 3] {
        &mut self.0
    }
}
