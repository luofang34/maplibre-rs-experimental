//! Native HTTP source loading with optional disk caching and typed retry classification.

use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
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

static SHARED_CACHE_FRONTENDS: Mutex<Option<HashMap<PathBuf, Weak<SharedCacheFrontend>>>> =
    Mutex::new(None);

fn shared_cache_frontend(path: &Path) -> Arc<SharedCacheFrontend> {
    let Ok(mut guard) = SHARED_CACHE_FRONTENDS.lock() else {
        return Arc::new(SharedCacheFrontend::new(MEMORY_CACHE_CAP_BYTES));
    };
    let table = guard.get_or_insert_with(HashMap::new);
    table.retain(|_, weak| weak.strong_count() > 0);
    if let Some(existing) = table.get(path).and_then(Weak::upgrade) {
        return existing;
    }
    let created = Arc::new(SharedCacheFrontend::new(MEMORY_CACHE_CAP_BYTES));
    table.insert(path.to_path_buf(), Arc::downgrade(&created));
    created
}

/// Cloneable HTTP transport that fetches complete bodies and preserves HTTP or network causes.
/// A configured cache directory enables the HTTP cache middleware; clones share the client.
#[derive(Clone)]
pub struct ReqwestHttpClient {
    client: ClientWithMiddleware,
    range_client: Client,
    frontend: Option<Arc<SharedCacheFrontend>>,
}

enum TransportClassification {
    NotFound(String),
    HttpResponse { url: String, status: u16 },
    Temporary,
    Terminal,
}

impl TransportClassification {
    fn into_fetch_error(self, source: impl std::error::Error + 'static) -> SourceFetchError {
        match self {
            Self::NotFound(url) => SourceFetchError::not_found(&url),
            Self::HttpResponse { url, status } => {
                SourceFetchError::http_response(&url, status, source)
            }
            Self::Temporary => SourceFetchError::temporary(source),
            Self::Terminal => SourceFetchError(Box::new(source)),
        }
    }
}

impl SourceFetchError {
    fn classify_reqwest_ref(err: &reqwest::Error) -> TransportClassification {
        if let Some(status) = err.status() {
            let url = err
                .url()
                .map_or("<unknown>", reqwest::Url::as_str)
                .to_owned();
            if status == StatusCode::NOT_FOUND {
                return TransportClassification::NotFound(url);
            }
            return TransportClassification::HttpResponse {
                url,
                status: status.as_u16(),
            };
        }
        if err.is_timeout() || err.is_connect() || err.is_request() || err.is_body() {
            return TransportClassification::Temporary;
        }
        classify_error_chain(std::error::Error::source(err))
    }
}

fn is_transient_io_error(err: &std::io::Error) -> bool {
    use std::io::ErrorKind;
    match err.kind() {
        ErrorKind::TimedOut
        | ErrorKind::ConnectionRefused
        | ErrorKind::ConnectionReset
        | ErrorKind::ConnectionAborted
        | ErrorKind::NotConnected
        | ErrorKind::AddrNotAvailable
        | ErrorKind::NetworkUnreachable
        | ErrorKind::HostUnreachable
        | ErrorKind::Interrupted
        | ErrorKind::WouldBlock
        | ErrorKind::UnexpectedEof => true,
        ErrorKind::Other => {
            let msg = err.to_string().to_ascii_lowercase();
            msg.contains("dns")
                || msg.contains("lookup address")
                || msg.contains("name resolution")
                || msg.contains("nodename nor servname")
                || msg.contains("name or service not known")
                || msg.contains("no address associated")
        }
        _ => false,
    }
}

fn classify_single_error(err: &(dyn std::error::Error + 'static)) -> TransportClassification {
    if let Some(mw_err) = err.downcast_ref::<reqwest_middleware::Error>() {
        return classify_middleware(mw_err);
    }
    if let Some(req_err) = err.downcast_ref::<reqwest::Error>() {
        return SourceFetchError::classify_reqwest_ref(req_err);
    }
    if let Some(io_err) = err.downcast_ref::<std::io::Error>() {
        if is_transient_io_error(io_err) {
            return TransportClassification::Temporary;
        }
    }
    TransportClassification::Terminal
}

fn classify_error_chain(
    mut current: Option<&(dyn std::error::Error + 'static)>,
) -> TransportClassification {
    while let Some(err) = current {
        let class = classify_single_error(err);
        if !matches!(class, TransportClassification::Terminal) {
            return class;
        }
        current = err.source();
    }
    TransportClassification::Terminal
}

fn classify_middleware(err: &reqwest_middleware::Error) -> TransportClassification {
    match err {
        reqwest_middleware::Error::Reqwest(req_err) => {
            SourceFetchError::classify_reqwest_ref(req_err)
        }
        reqwest_middleware::Error::Middleware(inner) => {
            if let Some(boxed) = inner.downcast_ref::<Box<dyn std::error::Error + Send + Sync>>() {
                let class = classify_error_chain(Some(boxed.as_ref()));
                if !matches!(class, TransportClassification::Terminal) {
                    return class;
                }
            }
            for cause in inner.chain() {
                let class = classify_single_error(cause);
                if !matches!(class, TransportClassification::Terminal) {
                    return class;
                }
            }
            classify_error_chain(std::error::Error::source(err))
        }
    }
}

impl From<reqwest::Error> for SourceFetchError {
    fn from(err: reqwest::Error) -> Self {
        SourceFetchError::classify_reqwest_ref(&err).into_fetch_error(err)
    }
}

impl From<reqwest_middleware::Error> for SourceFetchError {
    fn from(err: reqwest_middleware::Error) -> Self {
        match err {
            reqwest_middleware::Error::Reqwest(error) => error.into(),
            other => classify_middleware(&other).into_fetch_error(other),
        }
    }
}

impl ReqwestHttpClient {
    /// Creates a client, optionally storing cacheable responses under `cache_path`.
    /// `None` disables the disk cache. If client configuration fails, construction logs
    /// a warning and falls back to Reqwest's default client.
    pub fn new<P>(cache_path: Option<P>) -> Self
    where
        P: Into<PathBuf>,
    {
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
        let frontend = if let Some(cache_path) = cache_path {
            let path = cache_path.into();
            let shared = shared_cache_frontend(&path);
            builder = builder.with(Cache(HttpCache {
                mode: CacheMode::Default,
                manager: CACacheManager::new(path, false),
                options: HttpCacheOptions::default(),
            }));
            Some(shared)
        } else {
            None
        };
        let client = builder.build();

        Self {
            client,
            range_client,
            frontend,
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

/// An in-process HTTP handler for local test servers when network sockets are unavailable.
pub type InProcessHandler = std::sync::Arc<
    dyn Fn(
            &str,
            Option<ByteRange>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Vec<u8>, SourceFetchError>> + Send>,
        > + Send
        + Sync,
>;

static IN_PROCESS_HANDLERS: std::sync::RwLock<Vec<(String, InProcessHandler)>> =
    std::sync::RwLock::new(Vec::new());

/// Registers an in-process handler for URLs matching `prefix`.
pub fn register_in_process_handler(prefix: String, handler: InProcessHandler) {
    if let Ok(mut handlers) = IN_PROCESS_HANDLERS.write() {
        handlers.retain(|(p, _)| p != &prefix);
        handlers.push((prefix, handler));
    }
}

/// Unregisters an in-process handler by `prefix`.
pub fn unregister_in_process_handler(prefix: &str) {
    if let Ok(mut handlers) = IN_PROCESS_HANDLERS.write() {
        handlers.retain(|(p, _)| p != prefix);
    }
}

fn lookup_in_process_handler(url: &str) -> Option<InProcessHandler> {
    let handlers = IN_PROCESS_HANDLERS.read().ok()?;
    handlers
        .iter()
        .find(|(prefix, _)| url.starts_with(prefix))
        .map(|(_, handler)| handler.clone())
}

impl ReqwestHttpClient {
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
                Err(err) => match lookup_in_process_handler(url) {
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
                    Err(err) => match lookup_in_process_handler(url) {
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

#[cfg(test)]
mod tests;
