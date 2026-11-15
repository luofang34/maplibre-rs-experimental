//! In-flight asset request guard and waiter wake bookkeeping for [`AssetCache`].

use std::{any::Any, sync::Arc, task::Waker, time::Duration};

use instant::Instant;

use super::{AssetCache, AssetFailure, Entry, Slot, State};

/// The task chosen to fetch an asset; dropping it unfinished lets a waiter take over.
pub(super) struct Flight {
    pub(super) cache: AssetCache,
    pub(super) key: String,
    pub(super) flight: u64,
    pub(super) finished: bool,
}

impl Flight {
    pub(super) fn finish(
        mut self,
        result: Result<(Arc<dyn Any + Send + Sync>, usize), AssetFailure>,
    ) {
        self.finished = true;
        let waiters = {
            let mut state = self.cache.lock();
            let waiters = take_waiters(&mut state, &self.key, self.flight);
            let Some(waiters) = waiters else {
                return;
            };
            match result {
                Ok((value, bytes)) => {
                    state.tick = state.tick.wrapping_add(1);
                    let tick = state.tick;
                    state.bytes += bytes;
                    state.entries.insert(
                        self.key.clone(),
                        Entry {
                            slot: Slot::Ready { value, bytes },
                            last_used: tick,
                        },
                    );
                    AssetCache::evict_to_budget(
                        &mut state,
                        self.cache.0.budget_bytes,
                        self.cache.0.negative_ttl,
                        &self.key,
                    );
                }
                Err(failure) if waiters.is_empty() && failure.is_retryable() => {
                    state.entries.remove(&self.key);
                }
                Err(failure) => {
                    let tick = state.tick;
                    state.entries.insert(
                        self.key.clone(),
                        Entry {
                            slot: Slot::Failed {
                                failure,
                                at: Instant::now(),
                                flight: self.flight,
                            },
                            last_used: tick,
                        },
                    );
                }
            }
            waiters
        };
        wake(waiters);
    }
}

impl Drop for Flight {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let waiters = {
            let mut state = self.cache.lock();
            let waiters = take_waiters(&mut state, &self.key, self.flight);
            if waiters.is_some() {
                state.entries.remove(&self.key);
            }
            waiters
        };
        wake(waiters.unwrap_or_default());
    }
}

/// The waiters of `flight` when `key` is still loading under that flight.
pub(super) fn take_waiters(state: &mut State, key: &str, flight: u64) -> Option<Vec<Waker>> {
    match state.entries.get_mut(key).map(|entry| &mut entry.slot) {
        Some(Slot::Loading {
            flight: id,
            waiters,
        }) if *id == flight => Some(std::mem::take(waiters)),
        _ => None,
    }
}

pub(super) fn is_stale_failure(slot: &Slot, negative_ttl: Duration) -> bool {
    matches!(slot, Slot::Failed { failure, at, .. } if failure.is_retryable() || at.elapsed() >= negative_ttl)
}

pub(super) fn wake(waiters: Vec<Waker>) {
    waiters.into_iter().for_each(Waker::wake);
}
