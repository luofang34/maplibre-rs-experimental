//! Monotonic style revision tracking and cached worker snapshots.
//!
//! Each newly created or deserialized [`Style`] receives a distinct revision number from a
//! process-wide counter, and every style or source mutation bumps that revision while clearing
//! cached worker snapshots. Request systems and worker IPC reuse the cached `Arc<Style>` across
//! frames in O(1) without deep-cloning or re-serializing the full style document per frame.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

use rustc_hash::FxHashMap;

use crate::{style::Style, util::sync::lock_unpoisoned};

static NEXT_STYLE_REVISION: AtomicU64 = AtomicU64::new(1);

#[inline]
pub(crate) fn next_style_revision() -> u64 {
    NEXT_STYLE_REVISION.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug)]
struct WorkerSnapshots<S> {
    full: Option<Arc<S>>,
    filtered: Option<((bool, bool), Arc<S>)>,
    vector_layer_ids: Option<Arc<[String]>>,
    layer_index_by_id: Option<Arc<FxHashMap<String, usize>>>,
    symbol_leaders: Option<Arc<FxHashMap<String, String>>>,
    derived: Option<DerivedStyleFacts>,
    covering_bounds: Option<(u8, f64)>,
    has_symbol_layers: Option<bool>,
    has_raster_layer: Option<bool>,
    has_any_raster_type: Option<bool>,
    has_visible_raster_type: Option<bool>,
    has_raster_or_media_sources: Option<bool>,
    has_heatmap_layer: Option<bool>,
    has_dem_layer: Option<bool>,
    has_media_source: Option<bool>,
    has_vector_source_layers: Option<bool>,
    symbol_layer_indices: Option<Arc<[usize]>>,
    vector_tile_summary: Option<TileKindSummary>,
    raster_tile_summary: Option<TileKindSummary>,
    raster_source_layers: Option<RasterSourceLayerIndex>,
}

impl<S> Default for WorkerSnapshots<S> {
    fn default() -> Self {
        Self {
            full: None,
            filtered: None,
            vector_layer_ids: None,
            layer_index_by_id: None,
            symbol_leaders: None,
            derived: None,
            covering_bounds: None,
            has_symbol_layers: None,
            has_raster_layer: None,
            has_any_raster_type: None,
            has_visible_raster_type: None,
            has_raster_or_media_sources: None,
            has_heatmap_layer: None,
            has_dem_layer: None,
            has_media_source: None,
            has_vector_source_layers: None,
            symbol_layer_indices: None,
            vector_tile_summary: None,
            raster_tile_summary: None,
            raster_source_layers: None,
        }
    }
}

/// Cached `(source_name, layer_indices)` pairs grouping non-hidden raster-like layers per style revision.
pub(crate) type RasterSourceLayerIndex = Arc<[(Option<String>, Arc<[usize]>)]>;

/// Cached geographic bounding box `(west, south, east, north)` for source bounds checks.
pub(crate) type SourceBounds = (f64, f64, f64, f64);

/// Cached per-[`TileKind`](crate::io::tile_sources::TileKind) zoom bounds and combined geographic bounds.
#[derive(Clone, Debug)]
pub(crate) struct TileKindSummary {
    pub min_zoom: Option<u8>,
    pub max_zoom: Option<u8>,
    pub bounds: Option<Arc<[SourceBounds]>>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct DerivedStyleFacts {
    pub depends_on_overscaling: bool,
    pub elevated_headroom_meters: f64,
}

/// Monotonic revision and lazy worker-snapshot cache owned by [`Style`].
#[derive(Debug)]
pub struct StyleRevisionState<S> {
    revision: u64,
    snapshots: Mutex<WorkerSnapshots<S>>,
}

impl<S> Default for StyleRevisionState<S> {
    fn default() -> Self {
        Self {
            revision: next_style_revision(),
            snapshots: Mutex::new(WorkerSnapshots::default()),
        }
    }
}

impl<S> Clone for StyleRevisionState<S> {
    fn clone(&self) -> Self {
        // A pure clone of an unchanged style preserves its revision identity while starting with
        // an empty snapshot slot so `Arc<S>` snapshots never retain reference cycles.
        Self {
            revision: self.revision,
            snapshots: Mutex::new(WorkerSnapshots::default()),
        }
    }
}

impl<S> StyleRevisionState<S> {
    #[inline]
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn bump(&mut self) {
        self.revision = next_style_revision();
        if let Ok(guard) = self.snapshots.get_mut() {
            *guard = WorkerSnapshots::default();
        } else {
            *lock_unpoisoned(&self.snapshots) = WorkerSnapshots::default();
        }
    }
}

impl Style {
    /// Monotonic revision of this style document, bumped whenever layers, sources, images,
    /// transitions, feature-state, or global-state mutate.
    #[inline]
    pub fn revision(&self) -> u64 {
        self.revision_state.revision()
    }

    /// Invalidates cached worker snapshots and advances [`Self::revision`] after an in-place edit.
    #[inline]
    pub fn bump_revision(&mut self) {
        self.revision_state.bump();
    }

    /// Returns a shared `Arc<Style>` snapshot for background tile workers, constructing it at most
    /// once per style revision.
    pub(crate) fn worker_snapshot(&self) -> Arc<Style> {
        let mut guard = lock_unpoisoned(&self.revision_state.snapshots);
        if let Some(snapshot) = &guard.full {
            return snapshot.clone();
        }
        let snapshot = Arc::new(self.clone());
        guard.full = Some(snapshot.clone());
        snapshot
    }

    /// Returns a shared `Arc<Style>` snapshot filtered for `(vectors_disabled, symbols_disabled)`.
    pub(crate) fn filtered_worker_snapshot(
        &self,
        vectors_disabled: bool,
        symbols_disabled: bool,
    ) -> Arc<Style> {
        if !vectors_disabled && !symbols_disabled {
            return self.worker_snapshot();
        }
        let key = (vectors_disabled, symbols_disabled);
        let mut guard = lock_unpoisoned(&self.revision_state.snapshots);
        if let Some((cached_key, snapshot)) = &guard.filtered {
            if *cached_key == key {
                return snapshot.clone();
            }
        }
        let mut worker_style = self.clone();
        if symbols_disabled {
            worker_style.layers.retain(|layer| layer.type_ != "symbol");
        }
        if vectors_disabled {
            worker_style.layers.retain(|layer| layer.type_ == "symbol");
        }
        worker_style.bump_revision();
        let snapshot = Arc::new(worker_style);
        guard.filtered = Some((key, snapshot.clone()));
        snapshot
    }

    /// Cached ordered list of non-symbol vector layer IDs for the current style revision.
    pub(crate) fn cached_vector_layer_ids(&self) -> Arc<[String]> {
        let mut guard = lock_unpoisoned(&self.revision_state.snapshots);
        if let Some(ids) = &guard.vector_layer_ids {
            return ids.clone();
        }
        let ids: Arc<[String]> = self
            .layers
            .iter()
            .filter(|layer| {
                layer.type_ != "symbol" && crate::style::layer::from_vector_tiles(layer)
            })
            .map(|layer| layer.id.clone())
            .collect::<Vec<_>>()
            .into();
        guard.vector_layer_ids = Some(ids.clone());
        ids
    }

    /// Cached style-wide projection/overscaling facts recomputed only when [`Self::revision`] changes.
    pub(crate) fn cached_derived_facts(
        &self,
        compute: impl FnOnce(&Style) -> DerivedStyleFacts,
    ) -> DerivedStyleFacts {
        let mut guard = lock_unpoisoned(&self.revision_state.snapshots);
        if let Some(facts) = guard.derived {
            return facts;
        }
        let facts = compute(self);
        guard.derived = Some(facts);
        facts
    }

    /// Cached `id -> layer_index` map for [`Self::layers`], rebuilt only when [`Self::revision`]
    /// changes.
    pub(crate) fn cached_layer_index_by_id(&self) -> Arc<FxHashMap<String, usize>> {
        let mut guard = lock_unpoisoned(&self.revision_state.snapshots);
        if let Some(map) = &guard.layer_index_by_id {
            return map.clone();
        }
        let map: FxHashMap<String, usize> = self
            .layers
            .iter()
            .enumerate()
            .map(|(index, layer)| (layer.id.clone(), index))
            .collect();
        let cached = Arc::new(map);
        guard.layer_index_by_id = Some(cached.clone());
        cached
    }

    /// Cached symbol placement-group `member -> leader` map for the current style revision.
    pub(crate) fn cached_symbol_leaders(
        &self,
        compute: impl FnOnce(&Style) -> FxHashMap<String, String>,
    ) -> Arc<FxHashMap<String, String>> {
        let mut guard = lock_unpoisoned(&self.revision_state.snapshots);
        if let Some(leaders) = &guard.symbol_leaders {
            return leaders.clone();
        }
        let cached = Arc::new(compute(self));
        guard.symbol_leaders = Some(cached.clone());
        cached
    }

    /// Cached `(view_max_zoom, elevated_headroom_meters)` bounds recomputed at most once per revision.
    pub(crate) fn cached_covering_bounds(
        &self,
        compute: impl FnOnce(&Style) -> (u8, f64),
    ) -> (u8, f64) {
        let mut guard = lock_unpoisoned(&self.revision_state.snapshots);
        if let Some(bounds) = guard.covering_bounds {
            return bounds;
        }
        let bounds = compute(self);
        guard.covering_bounds = Some(bounds);
        bounds
    }

    /// Cached check for whether the style declares any `"symbol"` layer.
    pub(crate) fn cached_has_symbol_layers(&self) -> bool {
        let mut guard = lock_unpoisoned(&self.revision_state.snapshots);
        if let Some(has) = guard.has_symbol_layers {
            return has;
        }
        let has = self.layers.iter().any(|layer| layer.type_ == "symbol");
        guard.has_symbol_layers = Some(has);
        has
    }

    /// Cached check for whether the style declares any `"raster"` layer.
    pub(crate) fn cached_has_raster_layer(&self) -> bool {
        let mut guard = lock_unpoisoned(&self.revision_state.snapshots);
        if let Some(has) = guard.has_raster_layer {
            return has;
        }
        let has = self.layers.iter().any(|layer| layer.type_ == "raster");
        guard.has_raster_layer = Some(has);
        has
    }

    /// Cached check for whether the style declares any layer whose type is in `types`.
    pub(crate) fn cached_has_any_raster_type(&self, types: &[&str]) -> bool {
        let mut guard = lock_unpoisoned(&self.revision_state.snapshots);
        if let Some(has) = guard.has_any_raster_type {
            return has;
        }
        let has = self
            .layers
            .iter()
            .any(|layer| types.contains(&layer.type_.as_str()));
        guard.has_any_raster_type = Some(has);
        has
    }

    /// Cached check for whether the style declares any `"heatmap"` layer.
    pub(crate) fn cached_has_heatmap_layers(&self) -> bool {
        let mut guard = lock_unpoisoned(&self.revision_state.snapshots);
        if let Some(has) = guard.has_heatmap_layer {
            return has;
        }
        let has = self.layers.iter().any(|layer| {
            matches!(
                layer.paint,
                Some(crate::style::layer::LayerPaint::Heatmap(_))
            )
        });
        guard.has_heatmap_layer = Some(has);
        has
    }

    /// Cached check for whether the style declares any `"hillshade"` or `"color-relief"` layer.
    pub(crate) fn cached_has_dem_layers(&self) -> bool {
        let mut guard = lock_unpoisoned(&self.revision_state.snapshots);
        if let Some(has) = guard.has_dem_layer {
            return has;
        }
        let has = self
            .layers
            .iter()
            .any(|layer| matches!(layer.type_.as_str(), "hillshade" | "color-relief"));
        guard.has_dem_layer = Some(has);
        has
    }

    /// Cached check for whether the style declares any `image`, `video`, or `canvas` source.
    pub(crate) fn cached_has_media_sources(&self) -> bool {
        let mut guard = lock_unpoisoned(&self.revision_state.snapshots);
        if let Some(has) = guard.has_media_source {
            return has;
        }
        let has = self.sources.values().any(|s| {
            matches!(
                s,
                crate::style::source::Source::Image(_)
                    | crate::style::source::Source::Video(_)
                    | crate::style::source::Source::Canvas(_)
            )
        });
        guard.has_media_source = Some(has);
        has
    }

    /// Cached check for whether any layer in the style reads vector tile geometry.
    pub(crate) fn cached_has_vector_source_layers(&self) -> bool {
        let mut guard = lock_unpoisoned(&self.revision_state.snapshots);
        if let Some(has) = guard.has_vector_source_layers {
            return has;
        }
        let has = self.layers.iter().any(|layer| {
            !matches!(
                layer.type_.as_str(),
                "raster" | "hillshade" | "color-relief" | "background"
            )
        });
        guard.has_vector_source_layers = Some(has);
        has
    }

    /// Cached layer indices of all `"symbol"` layers in this style revision.
    pub(crate) fn cached_symbol_layer_indices(&self) -> Arc<[usize]> {
        let mut guard = lock_unpoisoned(&self.revision_state.snapshots);
        if let Some(indices) = &guard.symbol_layer_indices {
            return indices.clone();
        }
        let indices: Arc<[usize]> = self
            .layers
            .iter()
            .enumerate()
            .filter_map(|(idx, layer)| (layer.type_ == "symbol").then_some(idx))
            .collect::<Vec<_>>()
            .into();
        guard.symbol_layer_indices = Some(indices.clone());
        indices
    }

    /// Cached check for whether any non-hidden layer has a type in `types`.
    pub(crate) fn cached_has_visible_raster_type(&self, types: &[&str]) -> bool {
        let mut guard = lock_unpoisoned(&self.revision_state.snapshots);
        if let Some(has) = guard.has_visible_raster_type {
            return has;
        }
        let has = self
            .layers
            .iter()
            .any(|layer| types.contains(&layer.type_.as_str()) && !layer.is_hidden());
        guard.has_visible_raster_type = Some(has);
        has
    }

    /// Cached check for whether the style declares any raster, DEM, image, video, or canvas source.
    pub(crate) fn cached_has_raster_or_media_sources(&self) -> bool {
        let mut guard = lock_unpoisoned(&self.revision_state.snapshots);
        if let Some(has) = guard.has_raster_or_media_sources {
            return has;
        }
        let has = self.sources.values().any(|s| {
            matches!(
                s,
                crate::style::source::Source::Raster(_)
                    | crate::style::source::Source::RasterDem(_)
                    | crate::style::source::Source::Image(_)
                    | crate::style::source::Source::Video(_)
                    | crate::style::source::Source::Canvas(_)
            )
        });
        guard.has_raster_or_media_sources = Some(has);
        has
    }

    /// Cached zoom range and source bounds summary for vector or raster-like layers.
    pub(crate) fn cached_tile_kind_summary(
        &self,
        is_raster: bool,
        compute: impl FnOnce(&Self) -> TileKindSummary,
    ) -> TileKindSummary {
        let mut guard = lock_unpoisoned(&self.revision_state.snapshots);
        let slot = if is_raster {
            &guard.raster_tile_summary
        } else {
            &guard.vector_tile_summary
        };
        if let Some(summary) = slot {
            return summary.clone();
        }
        let summary = compute(self);
        if is_raster {
            guard.raster_tile_summary = Some(summary.clone());
        } else {
            guard.vector_tile_summary = Some(summary.clone());
        }
        summary
    }

    /// Cached non-hidden raster-like layer indices grouped by resolved source name.
    pub(crate) fn cached_raster_source_layers(
        &self,
        compute: impl FnOnce(&Self) -> Vec<(Option<String>, Vec<usize>)>,
    ) -> RasterSourceLayerIndex {
        let mut guard = lock_unpoisoned(&self.revision_state.snapshots);
        if let Some(groups) = &guard.raster_source_layers {
            return groups.clone();
        }
        let groups: RasterSourceLayerIndex = compute(self)
            .into_iter()
            .map(|(name, indices)| (name, Arc::from(indices)))
            .collect::<Vec<_>>()
            .into();
        guard.raster_source_layers = Some(groups.clone());
        groups
    }
}
