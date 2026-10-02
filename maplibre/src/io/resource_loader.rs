//! The one loader every request a map makes goes through, whichever thread makes it.
//!
//! A host injects a loader once; the map thread fetches TileJSON with it and native tile
//! workers fetch tiles, glyphs and sprites with clones of the same handle. Any
//! [`HttpClient`] is a loader, so a host can pass its own transport, an in-process fake, or an
//! adapter such as an archive reader.

use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex, PoisonError},
};

use async_trait::async_trait;
use futures::channel::oneshot;

use crate::io::source_client::{ByteRange, HttpClient, SourceFetchError};

/// Fetches whole resources and byte ranges of them.
///
/// Implemented by every [`HttpClient`]; a loader that is not `Clone` implements it directly.
#[cfg_attr(not(feature = "thread-safe-futures"), async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait)]
pub trait ResourceLoader: Send + Sync + 'static {
    /// The resource's body.
    async fn load(&self, url: &str) -> Result<Vec<u8>, SourceFetchError>;
    /// `range` of the resource's body.
    async fn load_range(&self, url: &str, range: ByteRange) -> Result<Vec<u8>, SourceFetchError>;
}

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait)]
impl<T: HttpClient> ResourceLoader for T {
    async fn load(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        self.fetch(url).await
    }

    async fn load_range(&self, url: &str, range: ByteRange) -> Result<Vec<u8>, SourceFetchError> {
        self.fetch_range(url, range).await
    }
}

/// A loader shared by the map thread and its workers; clones share one loader.
///
/// Requests for the same bytes while one is in flight wait for it instead of fetching again.
/// If the request they wait for is cancelled, the next of them fetches in its place.
#[derive(Clone)]
pub struct SharedLoader(Arc<Inner>);

struct Inner {
    loader: Box<dyn ResourceLoader>,
    in_flight: Mutex<HashMap<Key, Vec<oneshot::Sender<Outcome>>>>,
}

type Key = (String, Option<ByteRange>);

/// What the request in flight delivers to the requests waiting for it.
#[derive(Clone)]
enum Outcome {
    Body(Arc<[u8]>),
    Failed(Arc<Failure>),
}

/// A failure another request met, for a request that waited on it.
#[derive(Debug, thiserror::Error)]
#[error("{description}")]
struct Failure {
    description: String,
    not_found: bool,
    retryable: bool,
}

impl Outcome {
    fn new(result: &Result<Vec<u8>, SourceFetchError>) -> Self {
        match result {
            Ok(body) => Self::Body(Arc::from(body.as_slice())),
            Err(error) => Self::Failed(Arc::new(Failure {
                description: error.describe(),
                not_found: error.is_not_found(),
                retryable: error.is_retryable(),
            })),
        }
    }

    fn into_result(self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        match self {
            Self::Body(body) => Ok(body.to_vec()),
            Self::Failed(failure) if failure.not_found => Err(SourceFetchError::not_found(url)),
            Self::Failed(failure) if failure.retryable => {
                Err(SourceFetchError::temporary(SharedFailure(failure)))
            }
            Self::Failed(failure) => Err(SourceFetchError(Box::new(SharedFailure(failure)))),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error(transparent)]
struct SharedFailure(Arc<Failure>);

/// Removes the request from the in-flight table however its fetch ends; dropped by a
/// cancellation, it drops the waiters' senders, which wakes them to fetch for themselves.
struct Leader<'a> {
    inner: &'a Inner,
    key: Key,
    /// Set once the waiters are answered, after which a new request may own the key.
    finished: bool,
}

impl Leader<'_> {
    fn finish(mut self, outcome: Outcome) {
        self.finished = true;
        for waiter in self.inner.take(&self.key) {
            waiter.send(outcome.clone()).ok();
        }
    }
}

impl Drop for Leader<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.inner.take(&self.key);
        }
    }
}

impl Inner {
    fn table(&self) -> std::sync::MutexGuard<'_, HashMap<Key, Vec<oneshot::Sender<Outcome>>>> {
        self.in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn take(&self, key: &Key) -> Vec<oneshot::Sender<Outcome>> {
        self.table().remove(key).unwrap_or_default()
    }

    async fn load(&self, url: &str, range: Option<ByteRange>) -> Result<Vec<u8>, SourceFetchError> {
        let key = (url.to_owned(), range);
        loop {
            let waiting = {
                let mut table = self.table();
                match table.get_mut(&key) {
                    Some(waiters) => {
                        let (sender, receiver) = oneshot::channel();
                        waiters.push(sender);
                        Some(receiver)
                    }
                    None => {
                        table.insert(key.clone(), Vec::new());
                        None
                    }
                }
            };
            match waiting {
                Some(receiver) => match receiver.await {
                    Ok(outcome) => return outcome.into_result(url),
                    // The request waited on was cancelled; the next fetches in its place.
                    Err(oneshot::Canceled) => continue,
                },
                None => {
                    let leader = Leader {
                        inner: self,
                        key: key.clone(),
                        finished: false,
                    };
                    let result = match range {
                        Some(range) => self.loader.load_range(url, range).await,
                        None => self.loader.load(url).await,
                    };
                    leader.finish(Outcome::new(&result));
                    return result;
                }
            }
        }
    }
}

impl SharedLoader {
    /// Shares `loader` with everything that receives a clone.
    pub fn new(loader: impl ResourceLoader) -> Self {
        Self(Arc::new(Inner {
            loader: Box::new(loader),
            in_flight: Mutex::default(),
        }))
    }
}

impl fmt::Debug for SharedLoader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SharedLoader")
    }
}

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait)]
impl HttpClient for SharedLoader {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        self.0.load(url, None).await
    }

    async fn fetch_range(&self, url: &str, range: ByteRange) -> Result<Vec<u8>, SourceFetchError> {
        self.0.load(url, Some(range)).await
    }
}

#[cfg(test)]
mod tests;
