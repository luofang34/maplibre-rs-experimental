//! The one loader every request a map makes goes through, whichever thread makes it.
//!
//! A host injects a loader once; the map thread fetches TileJSON with it and native tile
//! workers fetch tiles, glyphs and sprites with clones of the same handle. Any
//! [`HttpClient`] is a loader, so a host can pass its own transport, an in-process fake, or an
//! adapter such as an archive reader.

use std::{fmt, sync::Arc};

use async_trait::async_trait;

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
#[derive(Clone)]
pub struct SharedLoader(Arc<dyn ResourceLoader>);

impl SharedLoader {
    /// Shares `loader` with everything that receives a clone.
    pub fn new(loader: impl ResourceLoader) -> Self {
        Self(Arc::new(loader))
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
        self.0.load(url).await
    }

    async fn fetch_range(&self, url: &str, range: ByteRange) -> Result<Vec<u8>, SourceFetchError> {
        self.0.load_range(url, range).await
    }
}

#[cfg(test)]
mod tests;
