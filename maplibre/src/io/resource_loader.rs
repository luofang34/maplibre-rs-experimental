//! The one loader every request a map makes goes through, whichever thread makes it.
//!
//! A host injects a loader once; the map thread fetches TileJSON with it and native tile
//! workers fetch tiles, glyphs and sprites with clones of the same handle. Any
//! [`HttpClient`] is a loader, so a host can pass its own transport or an in-process fake;
//! `pmtiles://` URLs are read out of their archive with range requests through it, and native
//! builds with `native-io` also serve local SQLite `mbtiles://` archives directly.

use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex, PoisonError},
};

use async_trait::async_trait;
use futures::channel::oneshot;

#[cfg(all(not(target_arch = "wasm32"), feature = "native-io"))]
use crate::io::mbtiles::{MbtilesReader, MbtilesUrl};
use crate::io::{
    pmtiles::{PmtilesReader, PmtilesUrl},
    source_client::{
        is_remote_http_url, request::decode_data_url, ByteRange, CachePolicy, HttpClient,
        HttpRequest, NetworkStatus, ResourceKind, SourceFetchError,
    },
};

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

    /// Executes a structured [`HttpRequest`], defaulting to [`Self::load_range`] or [`Self::load`].
    async fn load_request(&self, req: &HttpRequest) -> Result<Vec<u8>, SourceFetchError> {
        match req.range {
            Some(range) => self.load_range(&req.url, range).await,
            None => self.load(&req.url).await,
        }
    }

    /// Executes `req` under `status`, rejecting remote HTTP(S) misses when [`NetworkStatus::Offline`].
    async fn load_request_with_status(
        &self,
        req: &HttpRequest,
        status: NetworkStatus,
    ) -> Result<Vec<u8>, SourceFetchError> {
        if status.is_offline() && is_remote_http_url(&req.url) {
            return Err(SourceFetchError::offline(&req.url));
        }
        self.load_request(req).await
    }
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

    async fn load_request(&self, req: &HttpRequest) -> Result<Vec<u8>, SourceFetchError> {
        self.fetch_request(req).await
    }

    async fn load_request_with_status(
        &self,
        req: &HttpRequest,
        status: NetworkStatus,
    ) -> Result<Vec<u8>, SourceFetchError> {
        self.fetch_request_with_status(req, status).await
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
    pmtiles: PmtilesReader,
    #[cfg(all(not(target_arch = "wasm32"), feature = "native-io"))]
    mbtiles: MbtilesReader,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    url: String,
    range: Option<ByteRange>,
    headers: Vec<(String, String)>,
    cache_policy: CachePolicy,
    status: NetworkStatus,
}

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
    offline: bool,
    retryable: bool,
}

impl Outcome {
    fn new(result: &Result<Vec<u8>, SourceFetchError>) -> Self {
        match result {
            Ok(body) => Self::Body(Arc::from(body.as_slice())),
            Err(error) => Self::Failed(Arc::new(Failure {
                description: error.describe(),
                not_found: error.is_not_found(),
                offline: error.is_offline(),
                retryable: error.is_retryable(),
            })),
        }
    }

    fn into_result(self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        match self {
            Self::Body(body) => Ok(body.to_vec()),
            Self::Failed(failure) if failure.not_found => Err(SourceFetchError::not_found(url)),
            Self::Failed(failure) if failure.offline => Err(SourceFetchError::offline(url)),
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

    async fn load_request(
        &self,
        req: &HttpRequest,
        status: NetworkStatus,
    ) -> Result<Vec<u8>, SourceFetchError> {
        let key = Key {
            url: req.url.clone(),
            range: req.range,
            headers: req.headers.clone(),
            cache_policy: req.cache_policy,
            status,
        };
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
                    Ok(outcome) => return outcome.into_result(&req.url),
                    // The request waited on was cancelled; the next fetches in its place.
                    Err(oneshot::Canceled) => continue,
                },
                None => {
                    let leader = Leader {
                        inner: self,
                        key: key.clone(),
                        finished: false,
                    };
                    #[cfg(not(target_arch = "wasm32"))]
                    if let Some(raw_path) = req.url.strip_prefix("file://") {
                        let result = load_local_file(&req.url, raw_path, req.range);
                        leader.finish(Outcome::new(&result));
                        return result;
                    }
                    let result = self.loader.load_request_with_status(req, status).await;
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
            pmtiles: PmtilesReader::default(),
            #[cfg(all(not(target_arch = "wasm32"), feature = "native-io"))]
            mbtiles: MbtilesReader::default(),
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
        let req = HttpRequest::new(url, ResourceKind::infer_from_url(url));
        self.fetch_request_with_status(&req, NetworkStatus::Online)
            .await
    }

    async fn fetch_range(&self, url: &str, range: ByteRange) -> Result<Vec<u8>, SourceFetchError> {
        let req = HttpRequest::new(url, ResourceKind::infer_from_url(url)).with_range(range);
        self.fetch_request_with_status(&req, NetworkStatus::Online)
            .await
    }

    async fn fetch_request(&self, req: &HttpRequest) -> Result<Vec<u8>, SourceFetchError> {
        self.fetch_request_with_status(req, NetworkStatus::Online)
            .await
    }

    async fn fetch_request_with_status(
        &self,
        req: &HttpRequest,
        status: NetworkStatus,
    ) -> Result<Vec<u8>, SourceFetchError> {
        if let Some(local) = decode_data_url(&req.url, req.range) {
            return local;
        }
        // An archive's range requests come back through this loader, so concurrent readers of
        // one archive share them.
        if req.range.is_none() {
            if let Some(pmtiles_req) = PmtilesUrl::parse(&req.url) {
                return self
                    .0
                    .pmtiles
                    .fetch_with_context(self, req, status, pmtiles_req)
                    .await;
            }
        }
        #[cfg(all(not(target_arch = "wasm32"), feature = "native-io"))]
        if let Some(mbtiles_req) = MbtilesUrl::parse(&req.url) {
            return self.0.mbtiles.fetch_with_context(req, mbtiles_req).await;
        }
        self.0.load_request(req, status).await
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn load_local_file(
    url: &str,
    raw_path: &str,
    range: Option<ByteRange>,
) -> Result<Vec<u8>, SourceFetchError> {
    use std::io::{ErrorKind, Read, Seek, SeekFrom};

    use crate::io::source_client::RangeOutOfBounds;

    let stripped = raw_path.strip_prefix("localhost").unwrap_or(raw_path);
    #[cfg(windows)]
    let stripped = if stripped.as_bytes().get(2) == Some(&b':') && stripped.starts_with('/') {
        &stripped[1..]
    } else {
        stripped
    };
    let decoded = percent_decode_path(stripped);
    let mut file = std::fs::File::open(&decoded).map_err(|err| {
        if err.kind() == ErrorKind::NotFound {
            SourceFetchError::not_found(url)
        } else {
            SourceFetchError(Box::new(err))
        }
    })?;

    match range {
        None => {
            let mut body = Vec::new();
            file.read_to_end(&mut body)
                .map_err(|err| SourceFetchError(Box::new(err)))?;
            Ok(body)
        }
        Some(range) => {
            let size_u64 = file
                .metadata()
                .map_err(|err| SourceFetchError(Box::new(err)))?
                .len();
            let end = range.offset.saturating_add(range.length);
            if end > size_u64 {
                return Err(SourceFetchError(Box::new(RangeOutOfBounds {
                    url: url.to_owned(),
                    offset: range.offset,
                    end,
                    size: usize::try_from(size_u64).unwrap_or(usize::MAX),
                })));
            }
            let len =
                usize::try_from(range.length).map_err(|err| SourceFetchError(Box::new(err)))?;
            file.seek(SeekFrom::Start(range.offset))
                .map_err(|err| SourceFetchError(Box::new(err)))?;
            let mut buf = vec![0_u8; len];
            file.read_exact(&mut buf)
                .map_err(|err| SourceFetchError(Box::new(err)))?;
            Ok(buf)
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn percent_decode_path(input: &str) -> std::path::PathBuf {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0;
    while idx < bytes.len() {
        if bytes[idx] == b'%' && idx + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (
                (bytes[idx + 1] as char).to_digit(16),
                (bytes[idx + 2] as char).to_digit(16),
            ) {
                out.push(((hi << 4) | lo) as u8);
                idx += 3;
                continue;
            }
        }
        out.push(bytes[idx]);
        idx += 1;
    }
    std::path::PathBuf::from(String::from_utf8_lossy(&out).into_owned())
}

#[cfg(test)]
mod tests;
