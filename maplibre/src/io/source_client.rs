//! HTTP client.

use async_trait::async_trait;
use thiserror::Error;

use crate::{
    coords::WorldTileCoords,
    io::source_type::{InvalidTileCoords, SourceType},
    sdf::assets::{AssetCache, ImageProviders},
};

/// A closure that returns a HTTP client.
pub type HTTPClientFactory<HC> = dyn Fn() -> HC;

/// Fetches complete response bodies through a platform-specific HTTP transport.
///
/// Implementations distinguish missing resources with [`SourceFetchError::not_found`],
/// transient transport failures with [`SourceFetchError::temporary`], and status failures
/// with [`SourceFetchError::http_response`]. Unclassified errors are not retried.
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

/// Resolves tile URL templates and delegates requests to an HTTP transport.
#[derive(Clone)]
pub struct HttpSourceClient<HC>
where
    HC: HttpClient,
{
    inner_client: HC,
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

    /// Whether retrying can recover a transport failure, timeout, rate limit or server error.
    /// Unclassified failures, malformed URLs, decode errors and other client errors are terminal.
    pub fn is_retryable(&self) -> bool {
        self.0.is::<TemporarySourceError>()
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
        while let Some(error) = cause {
            text.push_str(": ");
            text.push_str(&error.to_string());
            cause = error.source();
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
}

impl<HC> HttpSourceClient<HC>
where
    HC: HttpClient,
{
    /// Wraps a transport without issuing requests.
    pub fn new(http_client: HC) -> Self {
        Self {
            inner_client: http_client,
        }
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
        tracing::debug!(%coords, %url, "fetching tile");
        self.inner_client.fetch(url.as_str()).await
    }

    /// Fetches an arbitrary URL through the inner client.
    pub async fn fetch_url(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        self.inner_client.fetch(url).await
    }
}

#[cfg(test)]
mod tests;
