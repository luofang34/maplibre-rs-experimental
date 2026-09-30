//! Decoded glyph ranges and sprite sheets shared by every tile a worker processes.
//!
//! Tiles that show the same font range or sprite sheet fetch and decode it once. Concurrent
//! requests for one asset wait for a single fetch instead of issuing their own. The cache is
//! shared by clones, so it spans calls only where the owner keeps one handle. A worker that
//! receives its configuration as a serialized message starts with its own cache; workers that
//! share memory and the same configuration value share one.

use std::{
    any::Any,
    collections::HashMap,
    future::{poll_fn, Future},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    task::{Poll, Waker},
};

use instant::{Duration, Instant};
use prost::Message;

use crate::{
    io::source_client::{HttpClient, SourceClient, SourceFetchError},
    sdf::glyphs::Glyphs,
};

/// Decoded assets are evicted least recently used once they exceed this many bytes.
const DEFAULT_BUDGET_BYTES: usize = 64 * 1024 * 1024;

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
    fn classify(error: &SourceFetchError) -> Self {
        if error.is_not_found() {
            Self::NotFound
        } else if error.is_retryable() {
            Self::Retryable(error.describe())
        } else {
            Self::Terminal(error.describe())
        }
    }

    fn is_retryable(&self) -> bool {
        matches!(self, Self::Retryable(_))
    }
}

/// A sprite sheet decoded once for every icon later cut from it.
pub(super) struct SpriteSheet {
    pub(super) document: HashMap<String, serde_json::Value>,
    pub(super) image: image::RgbaImage,
}

/// Shared handle to the glyph and sprite cache; clones use the same storage.
#[derive(Clone, Debug)]
pub struct AssetCache(Arc<Shared>);

struct Shared {
    state: Mutex<State>,
    budget_bytes: usize,
    negative_ttl: Duration,
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
    cache: AssetCache,
    key: String,
    flight: u64,
    finished: bool,
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
        }))
    }

    /// Decoded bytes currently held.
    pub fn approximate_bytes(&self) -> usize {
        self.lock().bytes
    }

    /// Fetches and decodes a glyph range, or returns the shared result of an earlier request.
    pub(super) async fn glyphs<HC: HttpClient>(
        &self,
        client: &SourceClient<HC>,
        url: &str,
    ) -> Result<Arc<Glyphs>, AssetFailure> {
        self.load(format!("glyphs:{url}"), || async {
            let bytes = fetch(client, url, "symbol glyph range").await?;
            decode_glyphs(&bytes, url)
        })
        .await
    }

    /// Decodes the glyph range compiled into the library, once.
    pub(super) async fn bundled_glyphs(
        &self,
        bytes: &'static [u8],
    ) -> Result<Arc<Glyphs>, AssetFailure> {
        self.load("glyphs:bundled".to_owned(), || async {
            decode_glyphs(bytes, "bundled glyph range")
        })
        .await
    }

    /// Fetches and decodes a sprite sheet, or returns the shared result of an earlier request.
    pub(super) async fn sprites<HC: HttpClient>(
        &self,
        client: &SourceClient<HC>,
        json_url: &str,
        png_url: &str,
    ) -> Result<Arc<SpriteSheet>, AssetFailure> {
        self.load(format!("sprites:{json_url}|{png_url}"), || async {
            let json = fetch(client, json_url, "symbol sprite metadata").await?;
            let png = fetch(client, png_url, "symbol sprite image").await?;
            let document = serde_json::from_slice(&json).map_err(|error| {
                tracing::warn!(%json_url, %error, "invalid sprite metadata");
                AssetFailure::Terminal(format!("invalid sprite metadata {json_url}: {error}"))
            })?;
            let image = image::load_from_memory(&png)
                .map_err(|error| {
                    tracing::warn!(%png_url, %error, "invalid sprite image");
                    AssetFailure::Terminal(format!("invalid sprite image {png_url}: {error}"))
                })?
                .to_rgba8();
            let bytes = image.as_raw().len() + json.len() * 4;
            Ok((SpriteSheet { document, image }, bytes))
        })
        .await
    }

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

    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.state.lock().unwrap_or_else(PoisonError::into_inner)
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
            cache: self.clone(),
            key: key.to_owned(),
            flight,
            finished: false,
        })
    }

    /// Waits until the flight ends; `Some` is a failure shared with every waiter of the flight,
    /// `None` means the caller should claim the asset again.
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

    fn evict_to_budget(state: &mut State, budget_bytes: usize, keep: &str) {
        while state.bytes > budget_bytes {
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

impl Flight {
    fn finish(mut self, result: Result<(Arc<dyn Any + Send + Sync>, usize), AssetFailure>) {
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
                    AssetCache::evict_to_budget(&mut state, self.cache.0.budget_bytes, &self.key);
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
fn take_waiters(state: &mut State, key: &str, flight: u64) -> Option<Vec<Waker>> {
    match state.entries.get_mut(key).map(|entry| &mut entry.slot) {
        Some(Slot::Loading {
            flight: id,
            waiters,
        }) if *id == flight => Some(std::mem::take(waiters)),
        _ => None,
    }
}

fn wake(waiters: Vec<Waker>) {
    for waker in waiters {
        waker.wake();
    }
}

pub(crate) async fn fetch<HC: HttpClient>(
    client: &SourceClient<HC>,
    url: &str,
    what: &str,
) -> Result<Vec<u8>, AssetFailure> {
    client.fetch_url(url).await.map_err(|error| {
        tracing::warn!(%url, error = %error.describe(), "{what} unavailable");
        AssetFailure::classify(&error)
    })
}

fn decode_glyphs(bytes: &[u8], source: &str) -> Result<(Glyphs, usize), AssetFailure> {
    let glyphs = Glyphs::decode(bytes).map_err(|error| {
        tracing::warn!(%source, %error, "invalid glyph range");
        AssetFailure::Terminal(format!("invalid glyph range {source}: {error}"))
    })?;
    let bytes = glyphs
        .stacks
        .iter()
        .flat_map(|stack| &stack.glyphs)
        .map(|glyph| std::mem::size_of_val(glyph) + glyph.bitmap.as_ref().map_or(0, Vec::len))
        .sum();
    Ok((glyphs, bytes))
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
