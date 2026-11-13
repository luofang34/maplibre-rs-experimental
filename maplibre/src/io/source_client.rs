//! HTTP client, request transform hooks, and connectivity-aware source fetching.

use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;

pub mod network;
pub mod request;

use self::request::{decode_data_url, TransformSlot};
pub use self::{
    network::{Connectivity, NetworkState, NetworkStatus, OfflineError},
    request::{
        is_remote_http_url, CachePolicy, DataUrlError, HttpRequest, RequestTransform, ResourceKind,
    },
};
use crate::{
    coords::WorldTileCoords,
    io::source_type::{InvalidTileCoords, SourceType},
    sdf::assets::{AssetCache, ImageProviders},
};

/// A closure that returns a HTTP client.
pub type HTTPClientFactory<HC> = dyn Fn() -> HC;

/// Fetches complete or partial response bodies through a platform-specific HTTP transport.
///
/// Implementations distinguish missing resources with [`SourceFetchError::not_found`],
/// transient transport failures with [`SourceFetchError::temporary`], offline blocks with
/// [`SourceFetchError::offline`], and status failures with [`SourceFetchError::http_response`].
/// Returned futures need `Send` only when `thread-safe-futures` is enabled; clients themselves
/// can be cloned and shared across threads.
#[cfg_attr(not(feature = "thread-safe-futures"), async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait)]
pub trait HttpClient: Clone + Sync + Send + 'static {
    /// Returns response bytes, or an error retaining the transport or HTTP failure cause.
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError>;

    /// Returns `range` of the response body. A transport that cannot ask for a range fetches
    /// the whole body and cuts the range out of it.
    async fn fetch_range(&self, url: &str, range: ByteRange) -> Result<Vec<u8>, SourceFetchError> {
        let body = self.fetch(url).await?;
        range.slice(url, &body).map(<[u8]>::to_vec)
    }

    /// Executes a structured [`HttpRequest`] carrying headers, range, and [`CachePolicy`].
    ///
    /// Defaults to delegating to [`Self::fetch_range`] or [`Self::fetch`] so existing
    /// [`HttpClient`] implementations remain source-compatible.
    async fn fetch_request(&self, req: &HttpRequest) -> Result<Vec<u8>, SourceFetchError> {
        match req.range {
            Some(range) => self.fetch_range(&req.url, range).await,
            None => self.fetch(&req.url).await,
        }
    }

    /// Executes `req` under the given [`NetworkStatus`].
    ///
    /// Cache-backed transports override this to serve in-memory, disk, or offline-store hits
    /// while [`NetworkStatus::Offline`] and reject remote misses with [`SourceFetchError::offline`].
    async fn fetch_request_with_status(
        &self,
        req: &HttpRequest,
        status: NetworkStatus,
    ) -> Result<Vec<u8>, SourceFetchError> {
        if status.is_offline() && is_remote_http_url(&req.url) {
            return Err(SourceFetchError::offline(&req.url));
        }
        self.fetch_request(req).await
    }
}

/// A span of a resource's bytes, as an HTTP `Range` header asks for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ByteRange {
    /// First byte.
    pub offset: u64,
    /// Number of bytes.
    pub length: u64,
}

/// A range reaches past the end of the resource it was asked of.
#[derive(Error, Debug)]
#[error("bytes {offset}..{end} are past the end of {url} ({size} bytes)")]
pub struct RangeOutOfBounds {
    /// The resource.
    pub url: String,
    /// First byte asked for.
    pub offset: u64,
    /// One past the last byte asked for.
    pub end: u64,
    /// The resource's size.
    pub size: usize,
}

impl ByteRange {
    /// The `Range` header value asking for these bytes.
    pub fn header(&self) -> String {
        format!(
            "bytes={}-{}",
            self.offset,
            self.offset + self.length.saturating_sub(1)
        )
    }

    /// These bytes of a whole body, or an error when the body is too short.
    pub fn slice<'a>(&self, url: &str, body: &'a [u8]) -> Result<&'a [u8], SourceFetchError> {
        let end = self.offset.saturating_add(self.length);
        usize::try_from(self.offset)
            .ok()
            .zip(usize::try_from(end).ok())
            .and_then(|(start, end)| body.get(start..end))
            .ok_or_else(|| {
                SourceFetchError(Box::new(RangeOutOfBounds {
                    url: url.to_owned(),
                    offset: self.offset,
                    end,
                    size: body.len(),
                }))
            })
    }
}

/// Resolves tile URL templates, applies [`RequestTransform`] hooks, enforces [`NetworkState`],
/// and delegates requests to an HTTP transport.
#[derive(Clone)]
pub struct HttpSourceClient<HC>
where
    HC: HttpClient,
{
    inner_client: HC,
    transform: TransformSlot,
    network: NetworkState,
}

/// A source URL could not be resolved or fetched; retains the underlying cause.
#[derive(Error, Debug)]
#[error("failed to fetch from source")]
pub struct SourceFetchError(#[source] pub Box<dyn std::error::Error>);

/// The server has no tile at the URL. Sources routinely omit tiles that hold no data, so a
/// request for one is answered like an empty tile rather than reported as a failure.
#[derive(Error, Debug)]
#[error("no tile at {url}")]
pub struct TileNotFound {
    /// The URL that answered 404.
    pub url: String,
}

/// An HTTP error response, retaining both response context and its transport error.
#[derive(Error, Debug)]
#[error("HTTP {status} from {url}")]
pub struct HttpResponseError {
    /// Request URL that returned the error status.
    pub url: String,
    /// HTTP response status code.
    pub status: u16,
    /// Original platform response error.
    #[source]
    pub source: Box<dyn std::error::Error>,
}

#[derive(Error, Debug)]
#[error("temporary source transport failure")]
struct TemporarySourceError(#[source] Box<dyn std::error::Error>);

impl SourceFetchError {
    /// Marks a transport failure as retryable while preserving its original cause.
    pub fn temporary(source: impl std::error::Error + 'static) -> Self {
        Self(Box::new(TemporarySourceError(Box::new(source))))
    }

    /// Records an HTTP status and its platform error for retry classification.
    pub fn http_response(url: &str, status: u16, source: impl std::error::Error + 'static) -> Self {
        Self(Box::new(HttpResponseError {
            url: url.to_owned(),
            status,
            source: Box::new(source),
        }))
    }

    /// Returns a retryable error indicating a remote URL was blocked because [`NetworkState`] is offline.
    pub fn offline(url: &str) -> Self {
        Self(Box::new(OfflineError {
            url: url.to_owned(),
        }))
    }

    /// Whether this failure resulted from [`NetworkStatus::Offline`] blocking a remote request.
    pub fn is_offline(&self) -> bool {
        self.0.downcast_ref::<OfflineError>().is_some()
    }

    /// Whether retrying can recover a transport failure, offline block, timeout, rate limit or server error.
    /// Unclassified failures, malformed URLs, decode errors and other client errors are terminal.
    pub fn is_retryable(&self) -> bool {
        self.0.is::<TemporarySourceError>()
            || self.is_offline()
            || self
                .0
                .downcast_ref::<HttpResponseError>()
                .is_some_and(|error| {
                    error.status == 408 || error.status == 429 || (500..600).contains(&error.status)
                })
    }

    /// The error for a URL the server answered with 404.
    pub fn not_found(url: &str) -> Self {
        Self(Box::new(TileNotFound {
            url: url.to_string(),
        }))
    }

    /// Whether the server answered that it has no such tile, which callers treat as an empty
    /// tile rather than a failure.
    pub fn is_not_found(&self) -> bool {
        self.0.downcast_ref::<TileNotFound>().is_some()
    }

    /// The message with every underlying cause appended, for single-line logs that would
    /// otherwise hide the HTTP status or connection failure behind the generic message.
    pub fn describe(&self) -> String {
        let mut text = self.to_string();
        let mut cause = std::error::Error::source(self);
        while let Some(next) = cause {
            let next_str = next.to_string();
            if !next_str.is_empty() && !text.contains(&next_str) {
                text.push_str(": ");
                text.push_str(&next_str);
            }
            cause = next.source();
        }
        text
    }
}

/// Fetches tile and document bytes through its configured HTTP source client.
#[derive(Clone)]
pub struct SourceClient<HC>
where
    HC: HttpClient,
{
    http: HttpSourceClient<HC>,
    assets: AssetCache,
    images: ImageProviders,
}

impl<HC> SourceClient<HC>
where
    HC: HttpClient,
{
    /// Creates a source client using the supplied HTTP adapter.
    pub fn new(http: HttpSourceClient<HC>) -> Self {
        Self {
            http,
            assets: AssetCache::default(),
            images: ImageProviders::default(),
        }
    }

    /// Borrows the underlying [`HttpSourceClient`].
    pub fn http_client(&self) -> &HttpSourceClient<HC> {
        &self.http
    }

    /// Attaches a [`RequestTransform`] hook shared across clones of this client.
    pub fn with_request_transform(
        self,
        transform: impl Fn(HttpRequest) -> HttpRequest + Send + Sync + 'static,
    ) -> Self {
        self.http.set_request_transform(transform);
        self
    }

    /// Installs or replaces the [`RequestTransform`] hook for this client and all its clones.
    pub fn set_request_transform(
        &self,
        transform: impl Fn(HttpRequest) -> HttpRequest + Send + Sync + 'static,
    ) {
        self.http.set_request_transform(transform);
    }

    /// Removes any installed [`RequestTransform`] hook.
    pub fn clear_request_transform(&self) {
        self.http.clear_request_transform();
    }

    /// Shares the supplied [`NetworkState`] handle with this client.
    pub fn with_network_state(mut self, network: NetworkState) -> Self {
        self.http = self.http.with_network_state(network);
        self
    }

    /// Returns the shared [`NetworkState`] handle governing online/offline transitions.
    pub fn network_state(&self) -> &NetworkState {
        self.http.network_state()
    }

    /// Updates online/offline status on this client's [`NetworkState`].
    pub fn set_online(&self, online: bool) {
        self.http.set_online(online);
    }

    /// Whether this client currently permits remote network sockets.
    pub fn is_online(&self) -> bool {
        self.http.is_online()
    }

    /// Shares decoded glyphs and sprites with every client that received the same handle.
    pub fn with_asset_cache(mut self, assets: AssetCache) -> Self {
        self.assets = assets;
        self
    }

    /// The cache of decoded symbol assets this client loads through.
    pub fn assets(&self) -> &AssetCache {
        &self.assets
    }

    /// Asks `images` for the provided images labels name.
    pub fn with_image_providers(mut self, images: ImageProviders) -> Self {
        self.images = images;
        self
    }

    /// The providers of images that labels name and no sprite supplies.
    pub fn image_providers(&self) -> &ImageProviders {
        &self.images
    }

    /// Resolves the source template for `coords` and fetches the response body.
    /// Returns [`InvalidTileCoords`] as a cause before making a request if addressing fails.
    pub async fn fetch(
        &self,
        coords: &WorldTileCoords,
        source_type: &SourceType,
    ) -> Result<Vec<u8>, SourceFetchError> {
        self.http.fetch(coords, source_type).await
    }

    /// Fetches a document that is not addressed by tile coordinates, such as TileJSON.
    pub async fn fetch_url(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        self.http.fetch_url(url).await
    }

    /// Fetches a source metadata document (such as TileJSON) respecting `volatile` cache policy.
    pub async fn fetch_source_url(
        &self,
        url: &str,
        volatile: bool,
    ) -> Result<Vec<u8>, SourceFetchError> {
        self.http.fetch_source_url(url, volatile).await
    }

    /// Executes an explicit [`HttpRequest`] through transform hooks and connectivity checks.
    pub async fn fetch_request(&self, req: HttpRequest) -> Result<Vec<u8>, SourceFetchError> {
        self.http.fetch_request(req).await
    }
}

impl<HC> HttpSourceClient<HC>
where
    HC: HttpClient,
{
    /// Wraps a transport without issuing requests.
    pub fn new(http_client: HC) -> Self {
        Self {
            inner_client: http_client,
            transform: TransformSlot::default(),
            network: NetworkState::new(),
        }
    }

    /// Borrows the wrapped inner [`HttpClient`].
    pub fn inner(&self) -> &HC {
        &self.inner_client
    }

    /// Configures a [`RequestTransform`] callback shared across clones of this client.
    pub fn with_request_transform(
        self,
        transform: impl Fn(HttpRequest) -> HttpRequest + Send + Sync + 'static,
    ) -> Self {
        self.set_request_transform(transform);
        self
    }

    /// Installs or replaces the [`RequestTransform`] callback on this client and all its clones.
    pub fn set_request_transform(
        &self,
        transform: impl Fn(HttpRequest) -> HttpRequest + Send + Sync + 'static,
    ) {
        self.transform.set(Arc::new(transform));
    }

    /// Installs a pre-boxed [`RequestTransform`] arc on this client and all its clones.
    pub fn set_request_transform_arc(&self, transform: RequestTransform) {
        self.transform.set(transform);
    }

    /// Clears any installed [`RequestTransform`] callback on this client and all its clones.
    pub fn clear_request_transform(&self) {
        self.transform.clear();
    }

    /// Replaces the [`NetworkState`] handle used by this client.
    pub fn with_network_state(mut self, network: NetworkState) -> Self {
        self.network = network;
        self
    }

    /// Borrows the shared [`NetworkState`] handle governing connectivity.
    pub fn network_state(&self) -> &NetworkState {
        &self.network
    }

    /// Toggles online (`true`) vs offline (`false`) mode on this client's [`NetworkState`].
    pub fn set_online(&self, online: bool) {
        self.network.set_online(online);
    }

    /// Whether remote HTTP/HTTPS network sockets are currently permitted.
    pub fn is_online(&self) -> bool {
        self.network.is_online()
    }

    /// Current [`Connectivity`] (`Online` vs `Offline`) of this client.
    pub fn connectivity(&self) -> Connectivity {
        self.network.status()
    }

    /// Applies any installed [`RequestTransform`] hook to `req`.
    pub fn apply_transform(&self, req: HttpRequest) -> HttpRequest {
        self.transform.apply(req)
    }

    /// Executes `req` after running [`RequestTransform`] and enforcing [`NetworkState`].
    pub async fn fetch_request(&self, req: HttpRequest) -> Result<Vec<u8>, SourceFetchError> {
        let req = self.apply_transform(req);
        if let Some(local) = decode_data_url(&req.url, req.range) {
            return local;
        }
        self.inner_client
            .fetch_request_with_status(&req, self.network.status())
            .await
    }

    /// Resolves the source template for `coords` and fetches the response body.
    /// Returns [`InvalidTileCoords`] as a cause before making a request if addressing fails.
    pub async fn fetch(
        &self,
        coords: &WorldTileCoords,
        source_type: &SourceType,
    ) -> Result<Vec<u8>, SourceFetchError> {
        let url = source_type
            .format(coords)
            .ok_or_else(|| SourceFetchError(Box::new(InvalidTileCoords { coords: *coords })))?;
        tracing::debug!(%coords, %url, volatile = source_type.is_volatile(), "fetching tile");
        let req = HttpRequest::new(url, ResourceKind::Tile)
            .with_cache_policy(CachePolicy::from_volatile(source_type.is_volatile()));
        self.fetch_request(req).await
    }

    /// Fetches an arbitrary URL through the inner client, inferring [`ResourceKind`] from the URL.
    pub async fn fetch_url(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        let req = HttpRequest::new(url, ResourceKind::infer_from_url(url));
        self.fetch_request(req).await
    }

    /// Fetches a source metadata document (such as TileJSON) with `volatile` cache policy.
    pub async fn fetch_source_url(
        &self,
        url: &str,
        volatile: bool,
    ) -> Result<Vec<u8>, SourceFetchError> {
        let req = HttpRequest::new(url, ResourceKind::Source)
            .with_cache_policy(CachePolicy::from_volatile(volatile));
        self.fetch_request(req).await
    }
}

#[cfg(test)]
mod tests;
