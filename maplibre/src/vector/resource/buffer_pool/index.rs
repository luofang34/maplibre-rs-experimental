use super::{page::FmStorageKind, *};

/// Metadata and buffer ranges of one uploaded tile style-layer allocation.
#[derive(Debug, Clone)]
pub struct IndexEntry {
    pub(super) allocation_id: u64,
    pub(super) paint_revision: u64,
    pub(super) vertex_page: u16,
    pub(super) index_page: u16,
    pub(super) layer_page: u16,
    pub(super) feature_page: u16,
    pub(super) index_u16: bool,
    pub(super) fm_kind: FmStorageKind,
    pub(super) shared_vertex_family: u8,
    /// Tile coordinates owning this layer geometry.
    pub coords: WorldTileCoords,
    /// Interned style layer identifier for `O(1)` equality checks during `Queue` and `Render`.
    pub style_layer_id: crate::style::StyleLayerId,
    /// Style layer definition at upload time.
    pub style_layer: std::sync::Arc<StyleLayer>,
    // Range of bytes within `vertex_slabs[vertex_page]` for vertices
    pub(super) buffer_vertices: Range<wgpu::BufferAddress>,
    // Range of bytes within `index_{u16,native}_slabs[index_page]` for indices
    pub(super) buffer_indices: Range<wgpu::BufferAddress>,
    // Range of bytes within `layer_slabs[layer_page]` for metadata
    pub(super) buffer_layer_metadata: Range<wgpu::BufferAddress>,
    // Range of bytes within `paired_fm` or `feature_slabs[feature_page]` for feature metadata
    pub(super) buffer_feature_metadata: Range<wgpu::BufferAddress>,
    // Element index of the first vertex within `vertex_slabs[vertex_page]`.
    pub(super) first_vertex: u32,
    // Element index of the first index within its index slab.
    pub(super) first_index: u32,
    // Amount of actually usable indices.
    pub(super) usable_indices: u32,
    // True when `buffer_feature_metadata` holds a single instance-stepped record (or 0 bytes for layer-uniform).
    pub(super) uniform_feature_metadata: bool,
    // True when `buffer_feature_metadata` stores two records per vertex (e.g. circle fill + stroke or fill + dynamic radius).
    pub(super) dual_feature_metadata: bool,
    // True when a `dual_feature_metadata` allocation carries `[fill_rgba8, dyn_radius_u32]`.
    pub(super) dynamic_circle_metadata: bool,
    // Shared source-layer vertex slab key when sibling layers share `buffer_vertices`.
    pub(super) shared_geometry_key: Option<u64>,
    // View zoom at which `feature_metadata` was last evaluated.
    pub(super) evaluated_zoom: Option<f32>,
    // Sub-ranges `(vertex_offset, local_index_range)` when > 65,536 vertices pack into `u16` indices.
    pub(super) u16_chunks: Option<super::u16_chunks::U16Chunks>,
    // True when `buffer_vertices` stores 8-byte compact extrusion records instead of 16-byte vertices.
    pub(super) compact_extrusion: bool,
    // True when all vertices of a solid-color fill draw with alpha == 1.0 (`blend: None` eligible).
    pub(super) opaque_color: bool,
}

impl IndexEntry {
    /// Whether this entry draws with fully opaque alpha (`1.0`) and can skip alpha blending.
    #[inline]
    pub fn is_opaque_color(&self) -> bool {
        self.opaque_color
    }

    /// Whether this entry stores 8-byte compact extrusion vertices (slot 0 stride `8`, no slot 3).
    #[inline]
    pub fn is_compact_extrusion(&self) -> bool {
        self.compact_extrusion
    }

    /// Whether this entry stores one `VertexStepMode::Instance` feature metadata record (or zero
    /// bytes when carried in `ShaderLayerMetadata`) instead of one record per vertex.
    #[inline]
    pub fn is_uniform_feature_metadata(&self) -> bool {
        self.uniform_feature_metadata
    }

    /// Whether this entry stores two feature metadata records per vertex.
    #[inline]
    pub fn is_dual_feature_metadata(&self) -> bool {
        self.dual_feature_metadata
    }

    /// Whether this entry stores `[fill_rgba8, dyn_radius_u32]` (`CircleDynamicRadiusShader`).
    #[inline]
    pub fn is_dynamic_circle_metadata(&self) -> bool {
        self.dynamic_circle_metadata
    }

    /// Whether indices for this entry are encoded as `u16` (`IndexFormat::Uint16`).
    #[inline]
    pub fn is_u16_indices(&self) -> bool {
        self.index_u16
    }

    #[inline]
    pub(super) fn has_paired_fm(&self) -> bool {
        self.fm_kind == FmStorageKind::Paired
    }

    /// Batch-scoped shared vertex-slab identifier when sibling style layers share `buffer_vertices`.
    #[inline]
    pub fn shared_geometry_key(&self) -> Option<u64> {
        self.shared_geometry_key
    }

    /// View zoom at which this entry's per-feature paint metadata was last evaluated.
    #[inline]
    pub fn evaluated_zoom(&self) -> Option<f32> {
        self.evaluated_zoom
    }

    /// Sub-ranges `(vertex_offset, local_index_range)` when `> 65,536` vertices pack into `u16`.
    #[inline]
    pub fn u16_chunks(&self) -> Option<&[(u32, Range<u32>)]> {
        self.u16_chunks.as_deref()
    }

    /// Resident GPU vertex, index, and feature-metadata bytes occupied by this allocation.
    #[inline]
    pub fn gpu_bytes(&self) -> usize {
        let v = self
            .buffer_vertices
            .end
            .saturating_sub(self.buffer_vertices.start);
        let i = self
            .buffer_indices
            .end
            .saturating_sub(self.buffer_indices.start);
        let fm = self
            .buffer_feature_metadata
            .end
            .saturating_sub(self.buffer_feature_metadata.start);
        usize::try_from(v.saturating_add(i).saturating_add(fm)).unwrap_or(usize::MAX)
    }

    /// Identifies this upload, even when an evicted layer reuses the same buffer range.
    pub fn allocation_id(&self) -> u64 {
        self.allocation_id
    }

    /// Combines [`Self::allocation_id`] with in-place paint/style revisions so cached terrain
    /// drapes and symbol placements refresh when GPU metadata changes without re-allocating.
    pub fn content_revision(&self) -> u64 {
        self.allocation_id
            .wrapping_add(self.paint_revision.wrapping_mul(0x9E37_79B9_7F4A_7C15))
    }

    /// Backing layer-metadata slab index (used with `layer_metadata_buffer_range().start` to
    /// deduplicate per-layer uniform updates across tiles).
    pub fn page(&self) -> u16 {
        self.layer_page
    }

    /// Zero-based local index range `0..usable_indices` when bound as a sub-slice.
    pub fn indices_range(&self) -> Range<u32> {
        0..self.usable_indices
    }

    /// The layer's indices within its index slab, offset by `base_vertex` when drawn.
    pub fn whole_buffer_indices(&self) -> Range<u32> {
        self.first_index..self.first_index + self.usable_indices
    }

    /// Added to every index of the layer when drawn from the whole vertex and paired feature
    /// metadata slabs.
    pub fn base_vertex(&self) -> i32 {
        self.first_vertex as i32
    }

    /// Byte range inside the entry's index slab.
    pub fn indices_buffer_range(&self) -> Range<wgpu::BufferAddress> {
        self.buffer_indices.clone()
    }

    /// Byte range inside the entry's vertex slab.
    pub fn vertices_buffer_range(&self) -> Range<wgpu::BufferAddress> {
        self.buffer_vertices.clone()
    }

    /// Byte range inside the entry's layer-metadata slab.
    pub fn layer_metadata_buffer_range(&self) -> Range<wgpu::BufferAddress> {
        self.buffer_layer_metadata.clone()
    }

    /// Byte range inside the entry's feature-metadata slab.
    pub fn feature_metadata_buffer_range(&self) -> Range<wgpu::BufferAddress> {
        self.buffer_feature_metadata.clone()
    }
}

/// Uploaded layer entries belonging to a single canonical tile.
#[derive(Debug)]
pub struct RingIndexEntry {
    layers: VecDeque<IndexEntry>,
}

/// Pyramid-backed spatial and temporal (`allocation_id`) index of resident geometry allocations.
#[derive(Debug)]
pub struct RingIndex {
    pyramid: TilePyramid<RingIndexEntry>,
    order: BTreeMap<u64, WorldTileCoords>,
}

impl RingIndex {
    /// Creates an empty index.
    pub fn new() -> Self {
        Self {
            pyramid: TilePyramid::new(),
            order: BTreeMap::new(),
        }
    }

    /// Hierarchical [`TilePyramid`] of resident tile geometry queues.
    #[inline]
    pub fn pyramid(&self) -> &TilePyramid<RingIndexEntry> {
        &self.pyramid
    }

    /// Removes all indexed entries in `O(1)` drop without per-entry rebalancing.
    pub fn clear(&mut self) {
        self.pyramid.clear();
        self.order.clear();
    }

    /// Returns `true` when zero geometry allocations reside in the pool.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// Oldest resident allocation in FIFO upload order.
    pub fn front(&self) -> Option<&IndexEntry> {
        let (&alloc_id, &coords) = self.order.first_key_value()?;
        self.pyramid
            .get(coords)?
            .layers
            .iter()
            .find(|entry| entry.allocation_id == alloc_id)
    }

    /// Newest resident allocation in FIFO upload order.
    pub fn back(&self) -> Option<&IndexEntry> {
        let (&alloc_id, &coords) = self.order.last_key_value()?;
        self.pyramid
            .get(coords)?
            .layers
            .iter()
            .rev()
            .find(|entry| entry.allocation_id == alloc_id)
    }

    /// Resident layer allocations at `coords`, if any.
    #[inline]
    pub fn get_layers(&self, coords: WorldTileCoords) -> Option<&VecDeque<IndexEntry>> {
        self.pyramid.get(coords).map(|entry| &entry.layers)
    }

    /// Resolves a resident [`IndexEntry`] in `O(1)` using `slot_hint`, falling back to integer
    /// [`StyleLayerId`](crate::style::StyleLayerId) scan only when the queue shifted.
    #[inline]
    pub fn resolve_entry(
        &self,
        coords: WorldTileCoords,
        style_layer_id: crate::style::StyleLayerId,
        slot_hint: u16,
    ) -> Option<&IndexEntry> {
        let layers = self.get_layers(coords)?;
        if slot_hint != crate::render::render_phase::UNSET_ENTRY_SLOT {
            if let Some(entry) = layers.get(usize::from(slot_hint)) {
                if entry.style_layer_id == style_layer_id {
                    return Some(entry);
                }
            }
        }
        layers
            .iter()
            .find(|entry| entry.style_layer_id == style_layer_id)
    }

    /// Highest zoom level among all resident allocations in `O(1)`.
    #[inline]
    pub fn max_zoom(&self) -> Option<u8> {
        self.pyramid.max_zoom()
    }

    /// Total resident GPU bytes (vertices, indices, feature metadata) across all layers at `coords`.
    #[inline]
    pub fn tile_gpu_bytes(&self, coords: WorldTileCoords) -> usize {
        self.get_layers(coords)
            .map_or(0, |layers| layers.iter().map(IndexEntry::gpu_bytes).sum())
    }

    /// Iterates per-tile layer queues across the index in coarse-to-fine Morton order.
    pub fn iter(&self) -> impl Iterator<Item = impl Iterator<Item = &IndexEntry>> + '_ {
        self.pyramid.values().map(|entry| entry.layers.iter())
    }

    pub(super) fn get_layers_mut(
        &mut self,
        coords: WorldTileCoords,
    ) -> Option<&mut VecDeque<IndexEntry>> {
        self.pyramid.get_mut(coords).map(|entry| &mut entry.layers)
    }

    pub(super) fn iter_mut(
        &mut self,
    ) -> impl Iterator<Item = impl Iterator<Item = &mut IndexEntry>> + '_ {
        self.pyramid
            .values_mut()
            .map(|entry| entry.layers.iter_mut())
    }

    /// Removes and returns the oldest resident allocation with `allocation_id >= *min_alloc_id`
    /// whose tile is not matched by `protect`, advancing `*min_alloc_id` past any skipped
    /// protected entries so repeated eviction steps stay `O(N)` total.
    pub(super) fn pop_oldest_from(
        &mut self,
        min_alloc_id: &mut u64,
        protect: &mut impl FnMut(WorldTileCoords) -> bool,
    ) -> Option<IndexEntry> {
        let (&alloc_id, &coords) = self
            .order
            .range(*min_alloc_id..)
            .find(|&(_, &coords)| !protect(coords))?;
        *min_alloc_id = alloc_id.saturating_add(1);
        self.order.remove(&alloc_id);
        let entry = self.pyramid.get_mut(coords)?;
        let pos = entry
            .layers
            .iter()
            .position(|layer| layer.allocation_id == alloc_id)?;
        let removed = entry.layers.remove(pos)?;
        if entry.layers.is_empty() {
            self.pyramid.remove(coords);
        }
        Some(removed)
    }

    pub(super) fn remove_tile(&mut self, coords: WorldTileCoords) -> Vec<IndexEntry> {
        let Some(entry) = self.pyramid.remove(coords) else {
            return Vec::new();
        };
        let mut removed = Vec::with_capacity(entry.layers.len());
        for layer in entry.layers {
            self.order.remove(&layer.allocation_id);
            removed.push(layer);
        }
        removed
    }

    pub(super) fn remove_layer(
        &mut self,
        coords: WorldTileCoords,
        id: crate::style::StyleLayerId,
        keep: Option<u64>,
    ) -> Vec<IndexEntry> {
        let Some(entry) = self.pyramid.get_mut(coords) else {
            return Vec::new();
        };
        let mut removed = Vec::new();
        let mut i = 0;
        while i < entry.layers.len() {
            let should_remove =
                entry.layers[i].style_layer_id == id && keep != Some(entry.layers[i].allocation_id);
            if should_remove {
                if let Some(layer) = entry.layers.remove(i) {
                    self.order.remove(&layer.allocation_id);
                    removed.push(layer);
                }
            } else {
                i += 1;
            }
        }
        if entry.layers.is_empty() {
            self.pyramid.remove(coords);
        }
        removed
    }

    pub(super) fn push_back(&mut self, entry: IndexEntry) {
        let coords = entry.coords;
        if coords.build_quad_key().is_none() {
            return;
        }
        self.order.insert(entry.allocation_id, coords);
        if let Some(existing) = self.pyramid.get_mut(coords) {
            existing.layers.push_back(entry);
        } else {
            self.pyramid.insert(
                coords,
                RingIndexEntry {
                    layers: VecDeque::from([entry]),
                },
            );
        }
    }
}

impl Default for RingIndex {
    fn default() -> Self {
        Self::new()
    }
}
