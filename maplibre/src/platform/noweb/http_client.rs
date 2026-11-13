//! Native HTTP source loading with optional disk caching and typed retry classification.

mod classify;

use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Mutex, RwLock},
};

use async_trait::async_trait;
use http_cache_reqwest::{CACacheManager, Cache, CacheMode, HttpCache, HttpCacheOptions};
use reqwest::{Client, StatusCode};
use reqwest_middleware::ClientWithMiddleware;
use tokio::sync::Semaphore;

use crate::io::source_client::{ByteRange, HttpClient, SourceFetchError};

const USER_AGENT: &str = concat!(
    "maplibre-rs/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/maplibre/maplibre-rs)"
);
const MEMORY_CACHE_CAP_BYTES: usize = 64 * 1024 * 1024;
const MEMORY_CACHE_MAX_ITEM_BYTES: usize = 1024 * 1024;
const DISK_CACHE_IO_CONCURRENCY: usize = 6;

struct MemoryTileCacheState {
    entries: HashMap<Arc<str>, (Arc<[u8]>, u64)>,
    order: VecDeque<(Arc<str>, u64)>,
    bytes: usize,
    clock: u64,
    cap_bytes: usize,
}

impl MemoryTileCacheState {
    fn new(cap_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            clock: 0,
            cap_bytes,
        }
    }

    fn get(&mut self, url: &str) -> Option<Arc<[u8]>> {
        let (key, (body, stamp)) = self.entries.get_key_value(url)?;
        let key = Arc::clone(key);
        let body = Arc::clone(body);
        let next_stamp = self.clock.wrapping_add(1);
        self.clock = next_stamp;
        if *stamp != next_stamp {
            if let Some(entry) = self.entries.get_mut(url) {
                entry.1 = next_stamp;
            }
            self.order.push_back((key, next_stamp));
            self.compact_order_if_needed();
        }
        Some(body)
    }

    fn insert(&mut self, url: &str, body: &[u8]) {
        if body.len() > MEMORY_CACHE_MAX_ITEM_BYTES || body.len() > self.cap_bytes {
            return;
        }
        let next_stamp = self.clock.wrapping_add(1);
        self.clock = next_stamp;
        let payload: Arc<[u8]> = Arc::from(body);
        if let Some((_, (old_body, _))) = self.entries.remove_entry(url) {
            self.bytes = self.bytes.saturating_sub(old_body.len());
        }
        while self.bytes.saturating_add(payload.len()) > self.cap_bytes {
            let Some((evict_key, evict_stamp)) = self.order.pop_front() else {
                self.entries.clear();
                self.bytes = 0;
                break;
            };
            if self
                .entries
                .get(evict_key.as_ref())
                .is_some_and(|(_, current_stamp)| *current_stamp == evict_stamp)
            {
                if let Some((evicted, _)) = self.entries.remove(evict_key.as_ref()) {
                    self.bytes = self.bytes.saturating_sub(evicted.len());
                }
            }
        }
        let key: Arc<str> = Arc::from(url);
        self.bytes = self.bytes.saturating_add(payload.len());
        self.entries.insert(Arc::clone(&key), (payload, next_stamp));
        self.order.push_back((key, next_stamp));
        self.compact_order_if_needed();
    }

    fn compact_order_if_needed(&mut self) {
        let limit = self.entries.len().saturating_mul(2).saturating_add(64);
        if self.order.len() <= limit {
            return;
        }
        let entries = &self.entries;
        self.order.retain(|(key, stamp)| {
            entries
                .get(key.as_ref())
                .is_some_and(|(_, current_stamp)| *current_stamp == *stamp)
        });
    }
}

struct SharedCacheFrontend {
    memory: Mutex<MemoryTileCacheState>,
    io_gate: Semaphore,
}

impl SharedCacheFrontend {
    fn new(cap_bytes: usize) -> Self {
        Self {
            memory: Mutex::new(MemoryTileCacheState::new(cap_bytes)),
            io_gate: Semaphore::new(DISK_CACHE_IO_CONCURRENCY),
        }
    }

    fn get(&self, url: &str) -> Option<Arc<[u8]>> {
        self.memory.lock().ok()?.get(url)
    }

    fn insert(&self, url: &str, body: &[u8]) {
        if let Ok(mut guard) = self.memory.lock() {
            guard.insert(url, body);
        }
    }
}

/// Shareable in-memory LRU cache and disk-I/O concurrency gate for [`ReqwestHttpClient`].
///
/// Cloning a [`ReqwestHttpClient`] already shares its own frontend; construct and pass an
/// [`HttpCacheFrontend`] explicitly when two independently built clients should share one.
#[derive(Clone)]
pub struct HttpCacheFrontend(Arc<SharedCacheFrontend>);

impl Default for HttpCacheFrontend {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpCacheFrontend {
    /// Creates a fresh in-memory cache frontend with the default 64 MiB budget.
    pub fn new() -> Self {
        Self(Arc::new(SharedCacheFrontend::new(MEMORY_CACHE_CAP_BYTES)))
    }
}

/// An in-process HTTP handler for local test servers when network sockets are unavailable.
pub type InProcessHandler = Arc<
    dyn Fn(
            &str,
            Option<ByteRange>,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, SourceFetchError>> + Send>>
        + Send
        + Sync,
>;

/// Instance-scoped registry of prefix-matched in-process fallback HTTP handlers.
#[derive(Clone, Default)]
pub struct InProcessRoutes(Arc<RwLock<Vec<(String, InProcessHandler)>>>);

impl InProcessRoutes {
    /// Creates an empty route table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers an in-process handler for URLs starting with `prefix`, replacing any prior
    /// handler registered for the exact same `prefix`.
    pub fn register(&self, prefix: impl Into<String>, handler: InProcessHandler) {
        let prefix = prefix.into();
        if let Ok(mut handlers) = self.0.write() {
            handlers.retain(|(existing, _)| existing != &prefix);
            handlers.push((prefix, handler));
        }
    }

    /// Removes any handler registered for `prefix`.
    pub fn unregister(&self, prefix: &str) {
        if let Ok(mut handlers) = self.0.write() {
            handlers.retain(|(existing, _)| existing != prefix);
        }
    }

    fn lookup(&self, url: &str) -> Option<InProcessHandler> {
        let handlers = self.0.read().ok()?;
        handlers
            .iter()
            .find(|(prefix, _)| url.starts_with(prefix))
            .map(|(_, handler)| Arc::clone(handler))
    }
}

/// Cloneable HTTP transport that fetches complete bodies and preserves HTTP or network causes.
/// A configured cache directory enables the HTTP cache middleware; clones share the client.
#[derive(Clone)]
pub struct ReqwestHttpClient {
    client: ClientWithMiddleware,
    range_client: Client,
    frontend: Option<Arc<SharedCacheFrontend>>,
    in_process: Option<InProcessRoutes>,
}

impl ReqwestHttpClient {
    /// Creates a client, optionally storing cacheable responses under `cache_path`.
    /// `None` disables the disk cache. If client configuration fails, construction logs
    /// a warning and falls back to Reqwest's default client.
    pub fn new<P>(cache_path: Option<P>) -> Self
    where
        P: Into<PathBuf>,
    {
        let cache_path = cache_path.map(Into::into);
        let frontend = cache_path
            .as_ref()
            .map(|_| Arc::new(SharedCacheFrontend::new(MEMORY_CACHE_CAP_BYTES)));
        Self::build(cache_path, frontend, None)
    }

    /// Creates a client whose in-memory cache and disk-I/O gate are shared through `frontend`.
    pub fn with_cache_frontend<P>(cache_path: Option<P>, frontend: HttpCacheFrontend) -> Self
    where
        P: Into<PathBuf>,
    {
        Self::build(cache_path.map(Into::into), Some(frontend.0), None)
    }

    /// Creates a client that consults `routes` when a network request cannot connect.
    pub fn with_in_process_routes<P>(cache_path: Option<P>, routes: InProcessRoutes) -> Self
    where
        P: Into<PathBuf>,
    {
        let cache_path = cache_path.map(Into::into);
        let frontend = cache_path
            .as_ref()
            .map(|_| Arc::new(SharedCacheFrontend::new(MEMORY_CACHE_CAP_BYTES)));
        Self::build(cache_path, frontend, Some(routes))
    }

    fn build(
        cache_path: Option<PathBuf>,
        frontend: Option<Arc<SharedCacheFrontend>>,
        in_process: Option<InProcessRoutes>,
    ) -> Self {
        // Public tile servers such as OpenStreetMap reject requests without a User-Agent.
        let client = match Client::builder().user_agent(USER_AGENT).build() {
            Ok(client) => client,
            Err(error) => {
                tracing::warn!(%error, "cannot build the HTTP client with a user agent");
                Client::new()
            }
        };
        // Byte-range requests (e.g. PMTiles archives) must read exact uncompressed byte offsets
        // (`Accept-Encoding: identity` with automatic gunzip disabled) and bypass `http-cache`,
        // whose cache key ignores `Range` headers and would otherwise serve the 127-byte header
        // slice for subsequent directory/tile ranges on the same URL.
        let range_client = match Client::builder().user_agent(USER_AGENT).no_gzip().build() {
            Ok(client) => client,
            Err(error) => {
                tracing::warn!(%error, "cannot build the range HTTP client with no_gzip");
                Client::new()
            }
        };
        let mut builder = reqwest_middleware::ClientBuilder::new(client);
        if let Some(path) = cache_path {
            builder = builder.with(Cache(HttpCache {
                mode: CacheMode::Default,
                manager: CACacheManager::new(path, false),
                options: HttpCacheOptions::default(),
            }));
        }
        Self {
            client: builder.build(),
            range_client,
            frontend,
            in_process,
        }
    }

    fn lookup_in_process_handler(&self, url: &str) -> Option<InProcessHandler> {
        self.in_process.as_ref()?.lookup(url)
    }

    /// A server that ignores the range answers with the whole body, cut down here.
    async fn get(&self, url: &str, range: Option<ByteRange>) -> Result<Vec<u8>, SourceFetchError> {
        let range_cache_key;
        let cache_key: &str = match range {
            Some(range) => {
                range_cache_key = format!("{url}#{}", range.header());
                &range_cache_key
            }
            None => url,
        };
        if let Some(body) = self
            .frontend
            .as_deref()
            .and_then(|frontend| frontend.get(cache_key))
        {
            return Ok(body.to_vec());
        }
        let response = match range {
            Some(range) => match self
                .range_client
                .get(url)
                .header(reqwest::header::RANGE, range.header())
                .header(reqwest::header::ACCEPT_ENCODING, "identity")
                .send()
                .await
            {
                Ok(response) => response,
                Err(err) => match self.lookup_in_process_handler(url) {
                    Some(handler) => return handler(url, Some(range)).await,
                    None => return Err(err.into()),
                },
            },
            None => {
                let _io_permit = match &self.frontend {
                    Some(frontend) => frontend.io_gate.acquire().await.ok(),
                    None => None,
                };
                match self.client.get(url).send().await {
                    Ok(response) => response,
                    Err(err) => match self.lookup_in_process_handler(url) {
                        Some(handler) => return handler(url, None).await,
                        None => return Err(err.into()),
                    },
                }
            }
        };
        if response.status() == StatusCode::NOT_FOUND {
            return Err(SourceFetchError::not_found(url));
        }
        let status = response.status().as_u16();
        match response.error_for_status() {
            Ok(response) => {
                if response.status() == StatusCode::NOT_MODIFIED {
                    log::info!("Using data from cache");
                }

                let partial = response.status() == StatusCode::PARTIAL_CONTENT;
                let body = response.bytes().await?;
                let out = match range {
                    Some(range) if !partial => range.slice(url, &body).map(<[u8]>::to_vec)?,
                    Some(_) | None => Vec::from(body.as_ref()),
                };
                if let Some(frontend) = &self.frontend {
                    frontend.insert(cache_key, &out);
                }
                Ok(out)
            }
            Err(error) => Err(SourceFetchError::http_response(url, status, error)),
        }
    }
}

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait)]
impl HttpClient for ReqwestHttpClient {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        self.get(url, None).await
    }

    async fn fetch_range(&self, url: &str, range: ByteRange) -> Result<Vec<u8>, SourceFetchError> {
        self.get(url, Some(range)).await
    }
}

#[cfg(test)]
mod tests;
