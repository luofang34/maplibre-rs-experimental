//! The browser `fetch` transport, usable from a page or a dedicated worker, with a two-tier
//! in-memory LRU + persistent Cache Storage tile cache.

pub mod cache;

use std::borrow::Cow;

use async_trait::async_trait;
use js_sys::{ArrayBuffer, Uint8Array};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Request, RequestCache, RequestInit, Response, WorkerGlobalScope};

use self::cache::{
    match_browser_cache, memory_cache_key, put_browser_cache, should_store_in_browser_cache,
};
pub use self::cache::{
    BrowserMemoryCache, BROWSER_MEMORY_CACHE_CAP_BYTES, BROWSER_TILE_CACHE_NAME,
};
use crate::io::{
    pmtiles::PmtilesClient,
    source_client::{
        is_remote_http_url, ByteRange, HttpClient, HttpRequest, NetworkStatus, ResourceKind,
        SourceFetchError,
    },
};

/// A browser request failed before a usable body arrived.
#[derive(thiserror::Error, Debug)]
pub enum FetchError {
    /// JavaScript threw or rejected; a network or CORS failure surfaces as a `TypeError` here.
    #[error("JavaScript error: {0}")]
    Js(Cow<'static, str>),
    /// `fetch` rejected without a response. Browsers report a refused connection, an offline
    /// page and a response the server's CORS headers withhold from this origin alike.
    #[error(
        "no response from {url} ({reason}): the network failed or the server does not allow \
         this origin (CORS)"
    )]
    Network {
        /// The requested resource.
        url: String,
        /// What JavaScript reported.
        reason: Cow<'static, str>,
    },
    /// The value JavaScript returned is not the type `fetch` promises.
    #[error("unexpected fetch value: {0}")]
    InvalidResponse(&'static str),
    /// The server answered with a status other than success or not found.
    #[error("HTTP {status}: {status_text}")]
    Status {
        /// The response's status code.
        status: u16,
        /// The response's status text.
        status_text: String,
    },
}

/// What a thrown or rejected JavaScript value says about itself.
fn js_message(value: &JsValue) -> Cow<'static, str> {
    value
        .dyn_ref::<js_sys::Error>()
        .and_then(|error| error.message().as_string())
        .or_else(|| value.as_string())
        .map_or(Cow::Borrowed("unknown JavaScript value"), Cow::Owned)
}

impl From<JsValue> for FetchError {
    fn from(value: JsValue) -> Self {
        Self::Js(js_message(&value))
    }
}

/// Fetches with the global `fetch` of the window or dedicated worker, backed by a shared
/// in-memory LRU byte cache and persistent browser `CacheStorage` (`"maplibre-rs-tiles-v1"`).
#[derive(Clone, Default)]
pub struct WHATWGFetchHttpClient {
    memory: BrowserMemoryCache,
}

/// The browser transport, also reading tiles out of PMTiles archives with range requests.
pub type WebHttpClient = PmtilesClient<WHATWGFetchHttpClient>;

/// A browser client with fresh in-memory LRU and PMTiles archive caches.
pub fn web_http_client() -> WebHttpClient {
    PmtilesClient::new(WHATWGFetchHttpClient::new())
}

fn rejected(error: JsValue) -> SourceFetchError {
    SourceFetchError(Box::new(FetchError::from(error)))
}

fn invalid_response(message: &'static str) -> SourceFetchError {
    SourceFetchError(Box::new(FetchError::InvalidResponse(message)))
}

struct WirePayload {
    bytes: Vec<u8>,
    status: u16,
    cache_control: Option<String>,
}

impl WHATWGFetchHttpClient {
    /// Creates a browser HTTP client with the default 32 MiB in-memory LRU cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a browser HTTP client backed by an explicit [`BrowserMemoryCache`].
    pub fn with_memory_cache(memory: BrowserMemoryCache) -> Self {
        Self { memory }
    }

    /// Borrows the shared in-memory LRU byte cache.
    pub fn memory_cache(&self) -> &BrowserMemoryCache {
        &self.memory
    }

    async fn fetch_wire(req: &HttpRequest) -> Result<WirePayload, SourceFetchError> {
        let url = req.url.as_str();
        let opts = RequestInit::new();
        opts.set_method("GET");
        if req.cache_policy.is_volatile() {
            opts.set_cache(RequestCache::NoStore);
        }
        let global = js_sys::global();
        let worker = global.dyn_into::<WorkerGlobalScope>().ok();
        if let Some(scope) = &worker {
            // A worker started from a `blob:` URL sends no `Referer`, and servers such as
            // OpenStreetMap's tile servers refuse requests without one. The page's origin is
            // what a main-thread fetch sends cross-origin, as GL JS's tile requests do.
            if scope.location().protocol() == "blob:" {
                opts.set_referrer(&format!("{}/", scope.origin()));
            }
        }
        let request = Request::new_with_str_and_init(url, &opts).map_err(rejected)?;
        let headers_obj = request.headers();
        for (name, value) in &req.headers {
            headers_obj.set(name, value).map_err(rejected)?;
        }
        if let Some(range) = req.range {
            headers_obj
                .set("Range", &range.header())
                .map_err(rejected)?;
        }

        // Tile workers and the main-thread TileJSON loader use the same transport.
        let promise = match worker {
            Some(scope) => scope.fetch_with_request(&request),
            None => web_sys::window()
                .ok_or_else(|| invalid_response("no worker or window scope to fetch from"))?
                .fetch_with_request(&request),
        };
        let response: Response = JsFuture::from(promise)
            .await
            .map_err(|error| {
                // Retried like any transient failure; a CORS refusal costs a backed-off retry.
                SourceFetchError::temporary(FetchError::Network {
                    url: url.to_owned(),
                    reason: js_message(&error),
                })
            })?
            .dyn_into()
            .map_err(|_| invalid_response("not a Response"))?;
        let status = response.status();
        if status == 404 {
            return Err(SourceFetchError::not_found(url));
        }
        if !response.ok() {
            return Err(SourceFetchError::http_response(
                url,
                status,
                FetchError::Status {
                    status,
                    status_text: response.status_text(),
                },
            ));
        }
        let cache_control = response.headers().get("cache-control").ok().flatten();
        let buffer = response.array_buffer().map_err(rejected)?;
        let body = JsFuture::from(buffer)
            .await
            .map_err(|error| SourceFetchError::temporary(FetchError::from(error)))?;
        let array_buffer: ArrayBuffer = body
            .dyn_into()
            .map_err(|_| invalid_response("not an ArrayBuffer"))?;
        let raw_bytes = Uint8Array::new(&array_buffer).to_vec();
        // A server that ignores `Range` answers with `200 OK` and the whole body; slice it here.
        let bytes = match req.range {
            Some(range) if status != 206 => range.slice(url, &raw_bytes).map(<[u8]>::to_vec)?,
            _ => raw_bytes,
        };
        Ok(WirePayload {
            bytes,
            status,
            cache_control,
        })
    }

    async fn execute(
        &self,
        req: &HttpRequest,
        status: NetworkStatus,
    ) -> Result<Vec<u8>, SourceFetchError> {
        let volatile = req.cache_policy.is_volatile();
        let mem_key = memory_cache_key(&req.url, req.range, &req.headers);

        if !volatile {
            if let Some(hit) = self.memory.get(&mem_key) {
                return Ok(hit.to_vec());
            }
            if req.range.is_none() && req.headers.is_empty() {
                if let Some(persisted) = match_browser_cache(&req.url).await {
                    self.memory.insert(&mem_key, &persisted);
                    return Ok(persisted);
                }
            }
        }

        if status.is_offline() && is_remote_http_url(&req.url) {
            return Err(SourceFetchError::offline(&req.url));
        }

        let payload = Self::fetch_wire(req).await?;
        if !volatile {
            self.memory.insert(&mem_key, &payload.bytes);
            if req.headers.is_empty()
                && should_store_in_browser_cache(
                    payload.status,
                    req.range,
                    req.cache_policy,
                    payload.cache_control.as_deref(),
                )
            {
                put_browser_cache(&req.url, &payload.bytes).await;
            }
        }
        Ok(payload.bytes)
    }
}

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait)]
impl HttpClient for WHATWGFetchHttpClient {
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
