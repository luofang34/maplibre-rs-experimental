//! Offline storage trait and region status records.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use super::memory::MemoryOfflineStore;
use super::region::OfflineRegionDefinition;
use crate::io::source_client::ResourceKind;

/// Stable identifier assigned to a stored offline region.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct OfflineRegionId(pub i64);

/// Accounting snapshot of resources stored for a single offline region.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfflineRegionStatus {
    /// Whether all required resources for this region have finished downloading.
    pub complete: bool,
    /// Number of resources currently linked to this region.
    pub completed_resource_count: u64,
    /// Uncompressed byte size of all resources linked to this region.
    pub completed_resource_bytes: u64,
    /// Number of tile resources (`ResourceKind::Tile`) linked to this region.
    pub completed_tile_count: u64,
    /// Total expected resource count once style and TileJSONs have been enumerated.
    pub required_resource_count: u64,
}

/// Summary pairing an [`OfflineRegionId`] with its [`OfflineRegionDefinition`] and [`OfflineRegionStatus`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OfflineRegionSummary {
    /// Assigned region identifier.
    pub id: OfflineRegionId,
    /// Geographic bounds, zoom range, style reference, and application metadata.
    pub definition: OfflineRegionDefinition,
    /// Current download completion and byte/tile accounting.
    pub status: OfflineRegionStatus,
}

/// Errors produced by an [`OfflineStore`] implementation.
#[derive(Error, Debug)]
pub enum OfflineStoreError {
    /// Requested region ID does not exist in the store.
    #[error("offline region {0:?} not found")]
    RegionNotFound(OfflineRegionId),
    /// Storing another offline tile would exceed the configured tile ceiling.
    #[error("offline tile count limit ({limit}) exceeded")]
    TileLimitExceeded {
        /// Configured ceiling on pinned offline tiles.
        limit: u64,
    },
    /// Serialization or deserialization of region definitions failed.
    #[error("offline store JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// Underlying I/O or decompression failure.
    #[error("offline store I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Blocking worker pool failure.
    #[error(transparent)]
    CpuPool(#[from] crate::io::cpu_work::CpuWorkError),
    /// Backend-specific database error message.
    #[error("offline database error: {0}")]
    Backend(String),
}

/// Storage backend for offline regions, deduplicated resources, and ambient cache entries.
#[cfg_attr(not(feature = "thread-safe-futures"), async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait)]
pub trait OfflineStore: Send + Sync + 'static {
    /// Persists a new region definition and returns its assigned [`OfflineRegionId`].
    async fn create_region(
        &self,
        definition: &OfflineRegionDefinition,
    ) -> Result<OfflineRegionId, OfflineStoreError>;

    /// Lists every region currently stored in ascending [`OfflineRegionId`] order.
    async fn list_regions(&self) -> Result<Vec<OfflineRegionSummary>, OfflineStoreError>;

    /// Loads the [`OfflineRegionDefinition`] for `id`.
    async fn region_definition(
        &self,
        id: OfflineRegionId,
    ) -> Result<OfflineRegionDefinition, OfflineStoreError>;

    /// Computes the current [`OfflineRegionStatus`] for `id`.
    async fn region_status(
        &self,
        id: OfflineRegionId,
    ) -> Result<OfflineRegionStatus, OfflineStoreError>;

    /// Updates the expected total resource count for `id`.
    async fn set_region_required_count(
        &self,
        id: OfflineRegionId,
        required_resource_count: u64,
    ) -> Result<(), OfflineStoreError>;

    /// Marks region `id` complete with `required_resource_count`.
    async fn mark_region_complete(
        &self,
        id: OfflineRegionId,
        required_resource_count: u64,
    ) -> Result<(), OfflineStoreError>;

    /// Retrieves a stored resource payload by `url`, returning `None` when absent.
    async fn get_resource(&self, url: &str) -> Result<Option<Vec<u8>>, OfflineStoreError>;

    /// Stores `data` for `url` and links it to `region_id`, deduplicating across overlapping regions.
    async fn put_region_resource(
        &self,
        region_id: OfflineRegionId,
        url: &str,
        kind: ResourceKind,
        data: &[u8],
    ) -> Result<(), OfflineStoreError>;

    /// Links an already-stored `url` to `region_id` if `url` is present, returning `true` on hit.
    async fn link_existing_resource(
        &self,
        region_id: OfflineRegionId,
        url: &str,
    ) -> Result<bool, OfflineStoreError>;

    /// Stores an unpinned ambient resource subject to [`Self::set_maximum_ambient_bytes`].
    async fn put_ambient_resource(
        &self,
        url: &str,
        kind: ResourceKind,
        data: &[u8],
    ) -> Result<(), OfflineStoreError>;

    /// Deletes region `id` and reclaims any non-ambient resources not referenced by other regions.
    async fn delete_region(&self, id: OfflineRegionId) -> Result<(), OfflineStoreError>;

    /// Marks region `id` incomplete, unlinks its resources, and reclaims unreferenced non-ambient bytes.
    async fn invalidate_region(&self, id: OfflineRegionId) -> Result<(), OfflineStoreError>;

    /// Sets the maximum byte budget for unpinned ambient cache entries, evicting oldest entries immediately.
    async fn set_maximum_ambient_bytes(&self, max_bytes: u64) -> Result<(), OfflineStoreError>;

    /// Sets an optional ceiling on total deduplicated tiles pinned across offline regions.
    async fn set_tile_count_limit(&self, limit: Option<u64>) -> Result<(), OfflineStoreError>;
}
