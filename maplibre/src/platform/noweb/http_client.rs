//! Native HTTP source loading with optional disk caching and typed retry classification.

mod cache;
mod classify;

use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, RwLock},
};

use async_trait::async_trait;
use http_cache_reqwest::{CACacheManager, Cache, CacheMode, HttpCache, HttpCacheOptions};
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue},
    Client, StatusCode,
};
use reqwest_middleware::ClientWithMiddleware;

pub use self::cache::HttpCacheFrontend;
#[cfg(test)]
use self::cache::MemoryTileCacheState;
use self::cache::{SharedCacheFrontend, MEMORY_CACHE_CAP_BYTES};
use crate::io::source_client::{
    is_remote_http_url, ByteRange, CachePolicy, HttpClient, HttpRequest, NetworkStatus,
    ResourceKind, SourceFetchError,
};

const USER_AGENT: &str = concat!(
    "maplibre-rs/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/maplibre/maplibre-rs)"
);

#[derive(Debug, thiserror::Error)]
#[error("invalid HTTP request header `{name}`: {reason}")]
struct InvalidHeaderError {
    name: String,
    reason: String,
}

fn build_header_map(headers: &[(String, String)]) -> Result<HeaderMap, SourceFetchError> {
    let mut map = HeaderMap::with_capacity(headers.len());
    for (name, value) in headers {
        let header_name = HeaderName::from_bytes(name.as_bytes()).map_err(|err| {
            SourceFetchError(Box::new(InvalidHeaderError {
                name: name.clone(),
                reason: err.to_string(),
            }))
        })?;
        let header_val = HeaderValue::from_str(value).map_err(|err| {
            SourceFetchError(Box::new(InvalidHeaderError {
                name: name.clone(),
                reason: err.to_string(),
            }))
        })?;
        map.append(header_name, header_val);
    }
    Ok(map)
}

fn format_cache_key(url: &str, range: Option<ByteRange>, headers: &[(String, String)]) -> String {
    let mut key = match range {
        Some(r) => format!("{url}#{}", r.header()),
        None => url.to_owned(),
    };
    if !headers.is_empty() {
        for (k, v) in headers {
            key.push('|');
            key.push_str(&k.to_ascii_lowercase());
            key.push(':');
            key.push_str(v);
        }
    }
    key
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
    offline_cache_client: Option<ClientWithMiddleware>,
    direct_client: Client,
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
        let direct_client = match Client::builder().user_agent(USER_AGENT).build() {
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
        let mut builder = reqwest_middleware::ClientBuilder::new(direct_client.clone());
        let offline_cache_client = cache_path.as_ref().map(|path| {
            reqwest_middleware::ClientBuilder::new(direct_client.clone())
                .with(Cache(HttpCache {
                    mode: CacheMode::OnlyIfCached,
                    manager: CACacheManager::new(path.clone(), false),
                    options: HttpCacheOptions::default(),
                }))
                .build()
        });
        if let Some(path) = cache_path {
            builder = builder.with(Cache(HttpCache {
                mode: CacheMode::Default,
                manager: CACacheManager::new(path, false),
                options: HttpCacheOptions::default(),
            }));
        }
        Self {
            client: builder.build(),
            offline_cache_client,
            direct_client,
            range_client,
            frontend,
            in_process,
        }
    }

    fn lookup_in_process_handler(&self, url: &str) -> Option<InProcessHandler> {
        self.in_process.as_ref()?.lookup(url)
    }

    /// Executes `req` under `status`. A server that ignores `Range` answers with the whole body,
    /// sliced down here.
    async fn execute(
        &self,
        req: &HttpRequest,
        status: NetworkStatus,
    ) -> Result<Vec<u8>, SourceFetchError> {
        let url = req.url.as_str();
        let range = req.range;
        let volatile = req.cache_policy.is_volatile();
        let header_map = build_header_map(&req.headers)?;
        let cache_key = format_cache_key(url, range, &req.headers);

        if !volatile {
            if let Some(body) = self
                .frontend
                .as_deref()
                .and_then(|frontend| frontend.get(&cache_key))
            {
                return Ok(body.to_vec());
            }
        }

        if status.is_offline() {
            return self.execute_offline(req, &header_map, &cache_key).await;
        }

        let response = match (range, volatile) {
            (Some(range), _) => {
                let request_builder = self
                    .range_client
                    .get(url)
                    .headers(header_map)
                    .header(reqwest::header::RANGE, range.header())
                    .header(reqwest::header::ACCEPT_ENCODING, "identity");
                match request_builder.send().await {
                    Ok(response) => response,
                    Err(err) => match self.lookup_in_process_handler(url) {
                        Some(handler) => return handler(url, Some(range)).await,
                        None => return Err(err.into()),
                    },
                }
            }
            (None, true) => {
                // Volatile sources bypass `CACacheManager` and `SharedCacheFrontend` completely.
                match self.direct_client.get(url).headers(header_map).send().await {
                    Ok(response) => response,
                    Err(err) => match self.lookup_in_process_handler(url) {
                        Some(handler) => return handler(url, None).await,
                        None => return Err(err.into()),
                    },
                }
            }
            (None, false) => {
                let _io_permit = match &self.frontend {
                    Some(frontend) => frontend.io_gate.acquire().await.ok(),
                    None => None,
                };
                match self.client.get(url).headers(header_map).send().await {
                    Ok(response) => response,
                    Err(err) => match self.lookup_in_process_handler(url) {
                        Some(handler) => return handler(url, None).await,
                        None => return Err(err.into()),
                    },
                }
            }
        };

        self.decode_response(url, range, req.cache_policy, &cache_key, response)
            .await
    }

    async fn execute_offline(
        &self,
        req: &HttpRequest,
        header_map: &HeaderMap,
        cache_key: &str,
    ) -> Result<Vec<u8>, SourceFetchError> {
        let url = req.url.as_str();
        if !req.cache_policy.is_volatile() && req.range.is_none() {
            if let Some(offline_client) = &self.offline_cache_client {
                let _io_permit = match &self.frontend {
                    Some(frontend) => frontend.io_gate.acquire().await.ok(),
                    None => None,
                };
                if let Ok(response) = offline_client
                    .get(url)
                    .headers(header_map.clone())
                    .send()
                    .await
                {
                    if response.status() != StatusCode::GATEWAY_TIMEOUT {
                        return self
                            .decode_response(url, None, req.cache_policy, cache_key, response)
                            .await;
                    }
                }
            }
        }
        if is_remote_http_url(url) {
            return Err(SourceFetchError::offline(url));
        }
        // Non-HTTP custom scheme fallback if registered in-process.
        if let Some(handler) = self.lookup_in_process_handler(url) {
            return handler(url, req.range).await;
        }
        Err(SourceFetchError::offline(url))
    }

    async fn decode_response(
        &self,
        url: &str,
        range: Option<ByteRange>,
        policy: CachePolicy,
        cache_key: &str,
        response: reqwest::Response,
    ) -> Result<Vec<u8>, SourceFetchError> {
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
                if !policy.is_volatile() {
                    if let Some(frontend) = &self.frontend {
                        frontend.insert(cache_key, &out);
                    }
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
        let req = HttpRequest::new(url, ResourceKind::infer_from_url(url));
        self.execute(&req, NetworkStatus::Online).await
    }

    async fn fetch_range(&self, url: &str, range: ByteRange) -> Result<Vec<u8>, SourceFetchError> {
        let req = HttpRequest::new(url, ResourceKind::infer_from_url(url)).with_range(range);
        self.execute(&req, NetworkStatus::Online).await
    }

    async fn fetch_request(&self, req: &HttpRequest) -> Result<Vec<u8>, SourceFetchError> {
        self.execute(req, NetworkStatus::Online).await
    }

    async fn fetch_request_with_status(
        &self,
        req: &HttpRequest,
        status: NetworkStatus,
    ) -> Result<Vec<u8>, SourceFetchError> {
        self.execute(req, status).await
    }
}

#[cfg(test)]
mod tests;
