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
        let mut builder = reqwest_middleware::ClientBuilder::new(client);
        let frontend = if let Some(cache_path) = cache_path {
            let path = cache_path.into();
            let shared = shared_cache_frontend(&path);
            builder = builder.with(Cache(HttpCache {
                mode: CacheMode::Default,
                manager: CACacheManager { path },
                options: HttpCacheOptions::default(),
            }));
            Some(shared)
        } else {
            None
        };
        let client = builder.build();

        Self { client, frontend }
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

impl ReqwestHttpClient {
    /// A server that ignores the range answers with the whole body, cut down here.
    async fn get(&self, url: &str, range: Option<ByteRange>) -> Result<Vec<u8>, SourceFetchError> {
        if range.is_none() {
            if let Some(body) = self
                .frontend
                .as_deref()
                .and_then(|frontend| frontend.get(url))
            {
                return Ok(body.to_vec());
            }
        }
        let _io_permit = match &self.frontend {
            Some(frontend) => frontend.io_gate.acquire().await.ok(),
            None => None,
        };
        let mut request = self.client.get(url);
        if let Some(range) = range {
            request = request.header(reqwest::header::RANGE, range.header());
        }
        let response = match request.send().await {
            Ok(response) => response,
            Err(err) => {
                let handler = {
                    let handlers = IN_PROCESS_HANDLERS.read().ok();
                    handlers.and_then(|h| {
                        h.iter()
                            .find(|(prefix, _)| url.starts_with(prefix))
                            .map(|(_, handler)| handler.clone())
                    })
                };
                if let Some(handler) = handler {
                    return handler(url, range).await;
                }
                return Err(err.into());
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
                match range {
                    Some(range) if !partial => range.slice(url, &body).map(<[u8]>::to_vec),
                    Some(_) => Ok(Vec::from(body.as_ref())),
                    None => {
                        if let Some(frontend) = &self.frontend {
                            frontend.insert(url, body.as_ref());
                        }
                        Ok(Vec::from(body.as_ref()))
                    }
                }
            }
            Err(error) => Err(SourceFetchError::http_response(url, status, error)),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::panic)]

    use std::io::ErrorKind;

    use super::*;

    #[derive(Debug, thiserror::Error)]
    #[error("middleware transport wrapper: {0}")]
    struct TransportWrapper(#[source] std::io::Error);

    #[derive(Debug, thiserror::Error)]
    #[error("cache metadata corrupt")]
    struct CacheMetadataCorrupt;

    #[test]
    fn middleware_wrapping_transient_io_errors_is_retryable() {
        for io_err in [
            std::io::Error::new(ErrorKind::TimedOut, "socket timed out"),
            std::io::Error::new(ErrorKind::ConnectionRefused, "connection refused"),
            std::io::Error::new(ErrorKind::ConnectionReset, "connection reset by peer"),
            std::io::Error::new(ErrorKind::NetworkUnreachable, "network unreachable"),
            std::io::Error::new(ErrorKind::HostUnreachable, "no route to host"),
            std::io::Error::other(
                "failed to lookup address information: nodename nor servname provided, or not known",
            ),
        ] {
            let mw_err = reqwest_middleware::Error::middleware(TransportWrapper(io_err));
            let fetch_err = SourceFetchError::from(mw_err);
            assert!(
                fetch_err.is_retryable(),
                "expected retryable for {}",
                fetch_err.describe()
            );
        }
    }

    #[test]
    fn non_network_middleware_errors_stay_non_retryable() {
        for mw_err in [
            reqwest_middleware::Error::middleware(CacheMetadataCorrupt),
            reqwest_middleware::Error::middleware(std::io::Error::new(
                ErrorKind::InvalidData,
                "invalid cache entry",
            )),
            reqwest_middleware::Error::middleware(std::io::Error::other(
                "unexpected header serialization error",
            )),
        ] {
            let fetch_err = SourceFetchError::from(mw_err);
            assert!(
                !fetch_err.is_retryable(),
                "expected non-retryable for {}",
                fetch_err.describe()
            );
        }
    }

    #[tokio::test]
    async fn cached_client_connect_or_dns_reqwest_error_in_middleware_is_retryable() {
        let raw_client = Client::builder()
            .timeout(std::time::Duration::from_millis(200))
            .build()
            .expect("reqwest client");
        let connect_err = raw_client
            .get("http://127.0.0.1:1/tiles/0/0/0.png")
            .send()
            .await
            .expect_err("closed local port must fail");

        let direct_mw = SourceFetchError::from(reqwest_middleware::Error::middleware(connect_err));
        assert!(
            direct_mw.is_retryable(),
            "direct middleware reqwest connect error must be retryable: {}",
            direct_mw.describe()
        );

        let cache_dir =
            std::env::temp_dir().join(format!("maplibre-http-cache-test-{}", std::process::id()));
        let cached_client = ReqwestHttpClient::new(Some(&cache_dir));
        let cached_err = cached_client
            .fetch("http://127.0.0.1:1/tiles/0/0/0.png")
            .await
            .expect_err("cached fetch against closed port must fail");
        let _ = std::fs::remove_dir_all(&cache_dir);
        assert!(
            cached_err.is_retryable(),
            "http-cache-reqwest wrapped transport error must be retryable: {}",
            cached_err.describe()
        );

        let bad_url_err = raw_client
            .get("://not-a-valid-url")
            .send()
            .await
            .expect_err("malformed URL must fail");
        let bad_url_mw = SourceFetchError::from(reqwest_middleware::Error::middleware(bad_url_err));
        assert!(
            !bad_url_mw.is_retryable(),
            "malformed URL builder error must remain terminal: {}",
            bad_url_mw.describe()
        );
    }

    #[test]
    fn memory_tile_cache_evicts_least_recently_used_entries_within_budget() {
        let mut cache = MemoryTileCacheState::new(10);
        cache.insert("a", &[1, 2, 3, 4]);
        cache.insert("b", &[5, 6, 7, 8]);
        assert_eq!(cache.get("a").as_deref(), Some(&[1, 2, 3, 4][..]));
        cache.insert("c", &[9, 10, 11, 12]);
        assert_eq!(cache.get("a").as_deref(), Some(&[1, 2, 3, 4][..]));
        assert!(cache.get("b").is_none());
        assert_eq!(cache.get("c").as_deref(), Some(&[9, 10, 11, 12][..]));
        assert!(cache.bytes <= 10);
    }
}
