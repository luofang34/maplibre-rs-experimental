//! Offline region definitions, persistent stores, background downloader, and cache-first transport.

mod enumerate;
pub mod manager;
mod memory;
pub mod region;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-io"))]
pub mod sqlite;
pub mod store;
pub mod transport;

#[cfg(all(not(target_arch = "wasm32"), feature = "native-io"))]
pub use self::sqlite::SqliteOfflineStore;
pub use self::{
    manager::{
        OfflineCancellationToken, OfflineDownloadError, OfflineDownloadProgress,
        OfflineDownloadState, OfflineManager,
    },
    region::{
        enumerate_style_resources, pending_non_volatile_tile_json_urls, OfflineRegionDefinition,
        OfflineResourceDescriptor,
    },
    store::{
        MemoryOfflineStore, OfflineRegionId, OfflineRegionStatus, OfflineRegionSummary,
        OfflineStore, OfflineStoreError,
    },
    transport::OfflineHttpClient,
};

#[cfg(test)]
mod tests;
