//! A loader for maps that draw only the tiles their host supplies.

use crate::io::source_client::{HttpClient, SourceFetchError};

/// Answers every request with "not found", for a map that must never touch the network.
///
/// Pass it as the loader of an offline map; its sources then fall back as for a missing tile.
#[derive(Clone)]
pub struct SuppliedTileClient;

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait::async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait::async_trait)]
impl HttpClient for SuppliedTileClient {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        Err(SourceFetchError::not_found(url))
    }
}
