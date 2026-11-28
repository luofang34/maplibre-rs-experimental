//! Shared async asset cache for decoded glyphs, sprite sheets, GeoJSON indices, and tile assets.

use std::{
    any::Any,
    future::{poll_fn, Future},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex, MutexGuard,
    },
    task::{Poll, Waker},
};

use instant::{Duration, Instant};
use rustc_hash::FxHashMap as HashMap;

use crate::io::source_client::SourceFetchError;

/// Decoded assets are evicted least recently used once they exceed this many bytes.
pub(crate) const DEFAULT_BUDGET_BYTES: usize = 1024 * 1024;

/// A missing or undecodable asset is not requested again for this long.
const NEGATIVE_TTL: Duration = Duration::from_secs(30);

/// Why an asset could not be loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssetFailure {
    /// The server has no such asset; the tile renders without it.
    NotFound,
    /// A transport failure or server error that a later attempt can recover from.
    Retryable(String),
    /// A response that cannot be used, such as a malformed glyph range or sprite image.
    Terminal(String),
}

impl AssetFailure {
    pub(crate) fn classify(error: &SourceFetchError) -> Self {
        if error.is_not_found() {
            Self::NotFound
        } else if error.is_retryable() {
            Self::Retryable(error.describe())
        } else {
            Self::Terminal(error.describe())
        }
    }

    /// Whether a later attempt can succeed.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Retryable(_))
    }
}

/// Shared handle to the decoded asset cache; clones use the same storage.
#[derive(Clone, Debug)]
pub struct AssetCache(Arc<Shared>);

struct Shared {
    state: Mutex<State>,
    budget_bytes: usize,
    negative_ttl: Duration,
    dem_decodes: AtomicUsize,
    raster_decodes: AtomicUsize,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        crate::util::sync::lock_unpoisoned(&self.state)
    }

    fn evict_to_budget(&self, state: &mut State, keep: &str) {
        if state.entries.len() > 256 {
            state.entries.retain(|key, entry| {
                key.as_str() == keep || !is_stale_failure(&entry.slot, self.negative_ttl)
            });
        }
        while state.bytes > self.budget_bytes {
            let oldest = state
                .entries
                .iter()
                .filter(|(key, entry)| {
                    key.as_str() != keep && matches!(entry.slot, Slot::Ready { .. })
                })
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone());
            let Some(oldest) = oldest else {
                return;
            };
            if let Some(Entry {
                slot: Slot::Ready { bytes, .. },
                ..
            }) = state.entries.remove(&oldest)
            {
                state.bytes = state.bytes.saturating_sub(bytes);
            }
        }
    }
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AssetCache")
            .field("budget_bytes", &self.budget_bytes)
            .finish_non_exhaustive()
    }
}

#[derive(Default)]
struct State {
    entries: HashMap<String, Entry>,
    bytes: usize,
    tick: u64,
    flights: u64,
}

struct Entry {
    slot: Slot,
    last_used: u64,
}

enum Slot {
    /// One task is fetching; the others wait on `waiters`.
    Loading { flight: u64, waiters: Vec<Waker> },
    Ready {
        value: Arc<dyn Any + Send + Sync>,
        bytes: usize,
    },
    /// Remembered so that repeated requests do not refetch. A retryable failure is handed to
    /// the tasks that waited on its flight and then treated as absent.
    Failed {
        failure: AssetFailure,
        at: Instant,
        flight: u64,
    },
}

enum Claim<T> {
    Ready(Arc<T>),
    Failed(AssetFailure),
    Wait(u64),
    Lead(Flight),
}

/// The task chosen to fetch an asset; dropping it unfinished lets a waiter take over.
struct Flight {
    shared: Arc<Shared>,
    key: String,
    flight: u64,
    finished: bool,
}

impl Flight {
    fn finish(mut self, result: Result<(Arc<dyn Any + Send + Sync>, usize), AssetFailure>) {
        self.finished = true;
        let waiters = {
            let mut state = self.shared.lock();
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
                    self.shared.evict_to_budget(&mut state, &self.key);
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
            let mut state = self.shared.lock();
            let waiters = take_waiters(&mut state, &self.key, self.flight);
            if waiters.is_some() {
                state.entries.remove(&self.key);
            }
            waiters
        };
        wake(waiters.unwrap_or_default());
    }
}

fn take_waiters(state: &mut State, key: &str, flight: u64) -> Option<Vec<Waker>> {
    match state.entries.get_mut(key).map(|entry| &mut entry.slot) {
        Some(Slot::Loading {
            flight: id,
            waiters,
        }) if *id == flight => Some(std::mem::take(waiters)),
        _ => None,
    }
}

fn is_stale_failure(slot: &Slot, negative_ttl: Duration) -> bool {
    matches!(slot, Slot::Failed { failure, at, .. } if failure.is_retryable() || at.elapsed() >= negative_ttl)
}

fn wake(waiters: Vec<Waker>) {
    waiters.into_iter().for_each(Waker::wake);
}

impl Default for AssetCache {
    fn default() -> Self {
        Self::with_budget(DEFAULT_BUDGET_BYTES)
    }
}

impl AssetCache {
    /// Creates a cache that keeps at most `budget_bytes` of decoded assets, except that the
    /// most recently loaded asset is always kept.
    pub fn with_budget(budget_bytes: usize) -> Self {
        Self::with_limits(budget_bytes, NEGATIVE_TTL)
    }

    /// Like [`Self::with_budget`], remembering a missing or malformed asset for `negative_ttl`.
    pub fn with_limits(budget_bytes: usize, negative_ttl: Duration) -> Self {
        Self(Arc::new(Shared {
            state: Mutex::new(State::default()),
            budget_bytes,
            negative_ttl,
            dem_decodes: AtomicUsize::new(0),
            raster_decodes: AtomicUsize::new(0),
        }))
    }

    /// Configured byte budget for decoded assets.
    pub(crate) fn budget_bytes(&self) -> usize {
        self.0.budget_bytes
    }

    /// Decoded bytes currently held.
    pub fn approximate_bytes(&self) -> usize {
        self.lock().bytes
    }

    /// Number of DEM tiles decoded through this cache.
    pub fn dem_decode_count(&self) -> usize {
        self.0.dem_decodes.load(Ordering::Relaxed)
    }

    /// Number of raster tile images decoded through this cache.
    pub fn raster_decode_count(&self) -> usize {
        self.0.raster_decodes.load(Ordering::Relaxed)
    }

    /// Increments the count of DEM tiles decoded through this cache.
    pub(crate) fn record_dem_decode(&self) {
        self.0.dem_decodes.fetch_add(1, Ordering::Relaxed);
    }

    /// Increments the count of raster tile images decoded through this cache.
    pub(crate) fn record_raster_decode(&self) {
        self.0.raster_decodes.fetch_add(1, Ordering::Relaxed);
    }

    /// Loads and caches an asset computed by `make` under `key`, coalescing concurrent callers.
    pub(crate) async fn load<T, F, Fut>(&self, key: String, make: F) -> Result<Arc<T>, AssetFailure>
    where
        T: Any + Send + Sync,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<(T, usize), AssetFailure>>,
    {
        let mut make = Some(make);
        loop {
            match self.claim::<T>(&key) {
                Claim::Ready(value) => return Ok(value),
                Claim::Failed(failure) => return Err(failure),
                Claim::Wait(flight) => {
                    if let Some(failure) = self.wait(&key, flight).await {
                        return Err(failure);
                    }
                }
                Claim::Lead(flight) => {
                    let Some(make) = make.take() else {
                        return Err(AssetFailure::Terminal("asset loader reused".to_owned()));
                    };
                    return match make().await {
                        Ok((value, bytes)) => {
                            let value = Arc::new(value);
                            let shared: Arc<dyn Any + Send + Sync> = value.clone();
                            flight.finish(Ok((shared, bytes)));
                            Ok(value)
                        }
                        Err(failure) => {
                            flight.finish(Err(failure.clone()));
                            Err(failure)
                        }
                    };
                }
            }
        }
    }

    /// Drops the settled asset under `key`, loaded or failed, so the next load fetches it
    /// again; a load in flight is left to finish for the tasks that wait on it.
    pub(crate) fn forget(&self, key: &str) {
        let mut state = self.lock();
        let bytes = match state.entries.get(key).map(|entry| &entry.slot) {
            Some(Slot::Ready { bytes, .. }) => *bytes,
            Some(Slot::Failed { .. }) => 0,
            Some(Slot::Loading { .. }) | None => return,
        };
        state.entries.remove(key);
        state.bytes = state.bytes.saturating_sub(bytes);
    }

    /// The asset under `key` if it is loaded, without loading it or waiting for a load.
    pub(crate) fn peek<T: Any + Send + Sync>(&self, key: &str) -> Option<Arc<T>> {
        let mut state = self.lock();
        state.tick = state.tick.wrapping_add(1);
        let tick = state.tick;
        match state.entries.get_mut(key) {
            Some(Entry {
                slot: Slot::Ready { value, .. },
                last_used,
            }) => {
                *last_used = tick;
                value.clone().downcast::<T>().ok()
            }
            _ => None,
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.lock()
    }

    fn claim<T: Any + Send + Sync>(&self, key: &str) -> Claim<T> {
        let mut state = self.lock();
        state.tick = state.tick.wrapping_add(1);
        let tick = state.tick;
        match state.entries.get_mut(key) {
            Some(Entry {
                slot: Slot::Ready { value, .. },
                last_used,
            }) => {
                *last_used = tick;
                return match value.clone().downcast::<T>() {
                    Ok(value) => Claim::Ready(value),
                    Err(_) => Claim::Failed(AssetFailure::Terminal(format!(
                        "cached asset {key} has another type"
                    ))),
                };
            }
            Some(Entry {
                slot: Slot::Loading { flight, .. },
                ..
            }) => return Claim::Wait(*flight),
            Some(Entry {
                slot: Slot::Failed { failure, at, .. },
                ..
            }) if !failure.is_retryable() && at.elapsed() < self.0.negative_ttl => {
                return Claim::Failed(failure.clone());
            }
            _ => {}
        }
        state.flights = state.flights.wrapping_add(1);
        let flight = state.flights;
        state.entries.insert(
            key.to_owned(),
            Entry {
                slot: Slot::Loading {
                    flight,
                    waiters: Vec::new(),
                },
                last_used: tick,
            },
        );
        Claim::Lead(Flight {
            shared: Arc::clone(&self.0),
            key: key.to_owned(),
            flight,
            finished: false,
        })
    }

    async fn wait(&self, key: &str, flight: u64) -> Option<AssetFailure> {
        poll_fn(|context| {
            let mut state = self.lock();
            match state.entries.get_mut(key).map(|entry| &mut entry.slot) {
                Some(Slot::Loading {
                    flight: id,
                    waiters,
                }) if *id == flight => {
                    waiters.push(context.waker().clone());
                    Poll::Pending
                }
                Some(Slot::Failed {
                    failure,
                    flight: id,
                    ..
                }) if *id == flight => Poll::Ready(Some(failure.clone())),
                _ => Poll::Ready(None),
            }
        })
        .await
    }
}
