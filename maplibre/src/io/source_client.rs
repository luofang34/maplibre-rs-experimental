//! HTTP client.

use async_trait::async_trait;
use thiserror::Error;

use crate::{
    coords::WorldTileCoords,
    io::source_type::{InvalidTileCoords, SourceType},
};

/// A closure that returns a HTTP client.
pub type HTTPClientFactory<HC> = dyn Fn() -> HC;

/// Fetches complete response bodies through a platform-specific HTTP transport.
///
/// Implementations distinguish missing resources with [`SourceFetchError::not_found`].
/// Returned futures need `Send` only when `thread-safe-futures` is enabled; clients themselves
/// can be cloned and shared across threads.
#[cfg_attr(not(feature = "thread-safe-futures"), async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait)]
pub trait HttpClient: Clone + Sync + Send + 'static {
    /// Returns response bytes, or an error retaining the transport or HTTP failure cause.
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError>;
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

impl SourceFetchError {
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
}

impl<HC> SourceClient<HC>
where
    HC: HttpClient,
{
    /// Creates a source client using the supplied HTTP adapter.
    pub fn new(http: HttpSourceClient<HC>) -> Self {
        Self { http }
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
