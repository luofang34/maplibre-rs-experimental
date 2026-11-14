//! Offline region download manager with bounded concurrency, progress streaming, and cancellation.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use futures::{channel::mpsc, StreamExt};
use thiserror::Error;

pub use super::transport::OfflineHttpClient;
use super::{
    region::{
        enumerate_style_resources, pending_non_volatile_tile_json_urls, OfflineRegionDefinition,
    },
    store::{OfflineRegionId, OfflineRegionSummary, OfflineStore, OfflineStoreError},
};
use crate::{
    io::{
        source_client::{HttpClient, HttpRequest, ResourceKind, SourceClient, SourceFetchError},
        tile_json::{apply_tile_json, apply_tile_json_to_dem, TileJson},
    },
    style::{source::Source, Style},
};

const DEFAULT_DOWNLOAD_CONCURRENCY: usize = 8;

/// Lifecycle state reported on [`OfflineDownloadProgress`] updates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OfflineDownloadState {
    /// Resources are actively being fetched and stored.
    Active,
    /// Every required resource has been persisted and the region is marked complete.
    Complete,
    /// Download stopped early via an [`OfflineCancellationToken`].
    Cancelled,
}

/// Progress snapshot emitted during an offline region download.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OfflineDownloadProgress {
    /// Identifier of the region being downloaded.
    pub region_id: OfflineRegionId,
    /// Number of resources completed so far (stored or verified sparse `404`).
    pub completed_resources: u64,
    /// Total expected resources for the region.
    pub required_resources: u64,
    /// Uncompressed bytes persisted for this region so far.
    pub completed_bytes: u64,
    /// Number of tile resources persisted for this region so far.
    pub completed_tiles: u64,
    /// Current lifecycle state (`Active`, `Complete`, or `Cancelled`).
    pub state: OfflineDownloadState,
}

/// Cooperative cancellation handle for aborting an in-flight offline region download.
#[derive(Clone, Default, Debug)]
pub struct OfflineCancellationToken(Arc<AtomicBool>);

impl OfflineCancellationToken {
    /// Creates an active (non-cancelled) token.
    pub fn new() -> Self {
        Self::default()
    }

    /// Signals cancellation to any download observing this token.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// Whether [`Self::cancel`] has been invoked.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Failure modes encountered while downloading an offline region.
#[derive(Error, Debug)]
pub enum OfflineDownloadError {
    /// Neither `style_json` nor `style_url` was set on the [`OfflineRegionDefinition`].
    #[error("offline region definition must specify either `style_json` or `style_url`")]
    MissingStyle,
    /// Parsing the root MapLibre style JSON or a source TileJSON document failed.
    #[error("invalid JSON document in offline region ({context}): {source}")]
    InvalidJson {
        /// URL or document label that failed parsing.
        context: String,
        /// Underlying JSON error.
        #[source]
        source: serde_json::Error,
    },
    /// Fetching a required resource over the source client failed.
    #[error("failed to fetch offline resource {url}: {source}")]
    Fetch {
        /// Target URL that failed.
        url: String,
        /// Transport error.
        #[source]
        source: SourceFetchError,
    },
    /// Persisting to or reading from the [`OfflineStore`] failed.
    #[error(transparent)]
    Store(#[from] OfflineStoreError),
    /// Download was cancelled via [`OfflineCancellationToken`].
    #[error("offline region {0:?} download cancelled")]
    Cancelled(OfflineRegionId),
}

/// Coordinates enumerating and downloading offline regions with bounded concurrency.
pub struct OfflineManager<HC: HttpClient, S: OfflineStore> {
    source_client: SourceClient<HC>,
    store: Arc<S>,
    concurrency: usize,
}

impl<HC: HttpClient, S: OfflineStore> Clone for OfflineManager<HC, S> {
    fn clone(&self) -> Self {
        Self {
            source_client: self.source_client.clone(),
            store: Arc::clone(&self.store),
            concurrency: self.concurrency,
        }
    }
}

impl<HC: HttpClient, S: OfflineStore> OfflineManager<HC, S> {
    /// Creates a manager using `source_client` for fetches and `store` for persistence.
    pub fn new(source_client: SourceClient<HC>, store: Arc<S>) -> Self {
        Self {
            source_client,
            store,
            concurrency: DEFAULT_DOWNLOAD_CONCURRENCY,
        }
    }

    /// Sets the maximum number of concurrent HTTP fetches per download (`>= 1`).
    pub fn with_concurrency(mut self, concurrency: usize) -> Self {
        self.concurrency = concurrency.max(1);
        self
    }

    /// Borrows the backing [`OfflineStore`].
    pub fn store(&self) -> &Arc<S> {
        &self.store
    }

    /// Creates and downloads a new offline region, forwarding progress updates over an unbounded channel.
    pub async fn download_region_to_channel(
        &self,
        definition: OfflineRegionDefinition,
        cancel: OfflineCancellationToken,
        progress_tx: mpsc::UnboundedSender<OfflineDownloadProgress>,
    ) -> Result<OfflineRegionSummary, OfflineDownloadError> {
        self.download_region(definition, cancel, move |prog| {
            let _ = progress_tx.unbounded_send(prog);
        })
        .await
    }

    /// Creates a new region record and downloads all style, TileJSON, sprite, glyph, and tile resources.
    pub async fn download_region(
        &self,
        definition: OfflineRegionDefinition,
        cancel: OfflineCancellationToken,
        on_progress: impl FnMut(OfflineDownloadProgress) + Send,
    ) -> Result<OfflineRegionSummary, OfflineDownloadError> {
        let region_id = self.store.create_region(&definition).await?;
        self.populate_region(region_id, definition, cancel, on_progress)
            .await
    }

    /// Refreshes or resumes an existing `region_id` (for instance after [`OfflineStore::invalidate_region`]).
    pub async fn refresh_region(
        &self,
        region_id: OfflineRegionId,
        cancel: OfflineCancellationToken,
        on_progress: impl FnMut(OfflineDownloadProgress) + Send,
    ) -> Result<OfflineRegionSummary, OfflineDownloadError> {
        let definition = self.store.region_definition(region_id).await?;
        self.populate_region(region_id, definition, cancel, on_progress)
            .await
    }

    async fn populate_region(
        &self,
        region_id: OfflineRegionId,
        definition: OfflineRegionDefinition,
        cancel: OfflineCancellationToken,
        mut on_progress: impl FnMut(OfflineDownloadProgress) + Send,
    ) -> Result<OfflineRegionSummary, OfflineDownloadError> {
        if cancel.is_cancelled() {
            on_progress(OfflineDownloadProgress {
                region_id,
                completed_resources: 0,
                required_resources: 0,
                completed_bytes: 0,
                completed_tiles: 0,
                state: OfflineDownloadState::Cancelled,
            });
            return Err(OfflineDownloadError::Cancelled(region_id));
        }

        let mut pre_stored: u64 = 0;
        let mut style = self
            .load_and_store_style(region_id, &definition, &mut pre_stored)
            .await?;

        self.resolve_and_store_tilejsons(region_id, &mut style, &mut pre_stored)
            .await?;

        let descriptors = enumerate_style_resources(&style, &definition);
        let required_total = pre_stored.saturating_add(descriptors.len() as u64);
        self.store
            .set_region_required_count(region_id, required_total)
            .await?;

        let initial_status = self.store.region_status(region_id).await?;
        let mut completed_resources = pre_stored;
        let mut completed_bytes = initial_status.completed_resource_bytes;
        let mut completed_tiles = initial_status.completed_tile_count;

        on_progress(OfflineDownloadProgress {
            region_id,
            completed_resources,
            required_resources: required_total,
            completed_bytes,
            completed_tiles,
            state: OfflineDownloadState::Active,
        });

        let client = self.source_client.clone();
        let store = Arc::clone(&self.store);
        let cancel_ref = cancel.clone();

        let mut stream = futures::stream::iter(descriptors)
            .map(|desc| {
                let client = client.clone();
                let store = Arc::clone(&store);
                let token = cancel_ref.clone();
                async move {
                    if token.is_cancelled() {
                        return Err(OfflineDownloadError::Cancelled(region_id));
                    }
                    let transformed = client
                        .http_client()
                        .apply_transform(HttpRequest::new(&desc.url, desc.kind));
                    let store_key = transformed.url.clone();

                    if store.link_existing_resource(region_id, &store_key).await? {
                        let sz = store
                            .get_resource(&store_key)
                            .await?
                            .map_or(0_u64, |b| b.len() as u64);
                        return Ok((desc.kind, sz, true));
                    }

                    match client
                        .fetch_request(HttpRequest::new(&desc.url, desc.kind))
                        .await
                    {
                        Ok(bytes) => {
                            let len = bytes.len() as u64;
                            store
                                .put_region_resource(region_id, &store_key, desc.kind, &bytes)
                                .await?;
                            Ok((desc.kind, len, true))
                        }
                        Err(err) if desc.optional_on_404 && err.is_not_found() => {
                            Ok((desc.kind, 0, false))
                        }
                        Err(source) => Err(OfflineDownloadError::Fetch {
                            url: desc.url,
                            source,
                        }),
                    }
                }
            })
            .buffer_unordered(self.concurrency);

        while let Some(item_result) = stream.next().await {
            if cancel.is_cancelled() {
                on_progress(OfflineDownloadProgress {
                    region_id,
                    completed_resources,
                    required_resources: required_total,
                    completed_bytes,
                    completed_tiles,
                    state: OfflineDownloadState::Cancelled,
                });
                return Err(OfflineDownloadError::Cancelled(region_id));
            }
            let (kind, byte_len, stored) = item_result?;
            completed_resources += 1;
            if stored {
                completed_bytes = completed_bytes.saturating_add(byte_len);
                if kind == ResourceKind::Tile {
                    completed_tiles += 1;
                }
            }
            on_progress(OfflineDownloadProgress {
                region_id,
                completed_resources,
                required_resources: required_total,
                completed_bytes,
                completed_tiles,
                state: OfflineDownloadState::Active,
            });
        }

        self.store
            .mark_region_complete(region_id, completed_resources)
            .await?;
        let final_status = self.store.region_status(region_id).await?;
        on_progress(OfflineDownloadProgress {
            region_id,
            completed_resources: final_status.completed_resource_count,
            required_resources: final_status.required_resource_count,
            completed_bytes: final_status.completed_resource_bytes,
            completed_tiles: final_status.completed_tile_count,
            state: OfflineDownloadState::Complete,
        });

        Ok(OfflineRegionSummary {
            id: region_id,
            definition,
            status: final_status,
        })
    }

    async fn load_and_store_style(
        &self,
        region_id: OfflineRegionId,
        definition: &OfflineRegionDefinition,
        pre_stored: &mut u64,
    ) -> Result<Style, OfflineDownloadError> {
        if let Some(style_json) = &definition.style_json {
            if let Some(style_url) = &definition.style_url {
                let key = self
                    .source_client
                    .http_client()
                    .apply_transform(HttpRequest::new(style_url, ResourceKind::Style))
                    .url;
                self.store
                    .put_region_resource(
                        region_id,
                        &key,
                        ResourceKind::Style,
                        style_json.as_bytes(),
                    )
                    .await?;
                *pre_stored += 1;
            }
            return serde_json::from_str::<Style>(style_json).map_err(|source| {
                OfflineDownloadError::InvalidJson {
                    context: "inline style_json".to_owned(),
                    source,
                }
            });
        }

        let Some(style_url) = &definition.style_url else {
            return Err(OfflineDownloadError::MissingStyle);
        };
        let key = self
            .source_client
            .http_client()
            .apply_transform(HttpRequest::new(style_url, ResourceKind::Style))
            .url;
        let bytes = match self.store.get_resource(&key).await? {
            Some(cached) => {
                self.store.link_existing_resource(region_id, &key).await?;
                cached
            }
            None => {
                let fetched = self
                    .source_client
                    .fetch_request(HttpRequest::new(style_url, ResourceKind::Style))
                    .await
                    .map_err(|source| OfflineDownloadError::Fetch {
                        url: style_url.clone(),
                        source,
                    })?;
                self.store
                    .put_region_resource(region_id, &key, ResourceKind::Style, &fetched)
                    .await?;
                fetched
            }
        };
        *pre_stored += 1;
        serde_json::from_slice::<Style>(&bytes).map_err(|source| {
            OfflineDownloadError::InvalidJson {
                context: style_url.clone(),
                source,
            }
        })
    }

    async fn resolve_and_store_tilejsons(
        &self,
        region_id: OfflineRegionId,
        style: &mut Style,
        pre_stored: &mut u64,
    ) -> Result<(), OfflineDownloadError> {
        let pending = pending_non_volatile_tile_json_urls(style);
        for (source_name, url) in pending {
            let key = self
                .source_client
                .http_client()
                .apply_transform(HttpRequest::new(&url, ResourceKind::Source))
                .url;
            let bytes = match self.store.get_resource(&key).await? {
                Some(cached) => {
                    self.store.link_existing_resource(region_id, &key).await?;
                    cached
                }
                None => {
                    let fetched = self
                        .source_client
                        .fetch_source_url(&url, false)
                        .await
                        .map_err(|source| OfflineDownloadError::Fetch {
                            url: url.clone(),
                            source,
                        })?;
                    self.store
                        .put_region_resource(region_id, &key, ResourceKind::Source, &fetched)
                        .await?;
                    fetched
                }
            };
            let _ = pre_stored; // TileJSON URL is also included in `enumerate_style_resources` via `link_existing_resource`.
            let tile_json: TileJson = serde_json::from_slice(&bytes).map_err(|source| {
                OfflineDownloadError::InvalidJson {
                    context: url,
                    source,
                }
            })?;
            if let Some(src) = style.sources.get_mut(&source_name) {
                match src {
                    Source::Vector(v) | Source::Raster(v) => apply_tile_json(v, tile_json),
                    Source::RasterDem(d) => apply_tile_json_to_dem(d, tile_json),
                    Source::GeoJson(_)
                    | Source::Image(_)
                    | Source::Video(_)
                    | Source::Canvas(_) => {}
                }
            }
        }
        Ok(())
    }
}
