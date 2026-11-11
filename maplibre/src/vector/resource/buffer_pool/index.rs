use super::*;

/// Metadata and buffer ranges of one uploaded tile style-layer allocation.
#[derive(Debug, Clone)]
pub struct IndexEntry {
    pub(super) allocation_id: u64,
    pub(super) page: u16,
    /// Tile coordinates owning this layer geometry.
    pub coords: WorldTileCoords,
    /// Style layer definition at upload time.
    pub style_layer: StyleLayer,
    // Range of bytes within the page's backing buffer for vertices
    pub(super) buffer_vertices: Range<wgpu::BufferAddress>,
    // Range of bytes within the page's backing buffer for indices
    pub(super) buffer_indices: Range<wgpu::BufferAddress>,
    // Range of bytes within the page's backing buffer for metadata
    pub(super) buffer_layer_metadata: Range<wgpu::BufferAddress>,
    // Range of bytes within the page's backing buffer for feature metadata
    pub(super) buffer_feature_metadata: Range<wgpu::BufferAddress>,
    // Element index of the first vertex within its page, which is also that of its first feature
    // metadata record: drawn from the whole page buffers with this as the base vertex.
    pub(super) first_vertex: u32,
    // Element index of the first index within its page's index buffer.
    pub(super) first_index: u32,
    // Amount of actually usable indices. Each index has the size/format `IndexDataType`.
    // Can be lower than size(buffer_indices) / indices_stride because of alignment.
    pub(super) usable_indices: u32,
}

impl IndexEntry {
    /// Identifies this upload, even when an evicted layer reuses the same buffer range.
    pub fn allocation_id(&self) -> u64 {
        self.allocation_id
    }

    /// Backing page slot holding this entry's vertex, index, and metadata buffers.
    pub fn page(&self) -> u16 {
        self.page
    }

    /// Zero-based local index range `0..usable_indices` when bound as a sub-slice.
    pub fn indices_range(&self) -> Range<u32> {
        0..self.usable_indices
    }

    /// The layer's indices within its page's index buffer, offset by `base_vertex` when drawn.
    pub fn whole_buffer_indices(&self) -> Range<u32> {
        self.first_index..self.first_index + self.usable_indices
    }

    /// Added to every index of the layer when drawn from the whole page vertex and feature
    /// metadata buffers.
    pub fn base_vertex(&self) -> i32 {
        self.first_vertex as i32
    }

    /// Byte range inside the entry's page index buffer.
    pub fn indices_buffer_range(&self) -> Range<wgpu::BufferAddress> {
        self.buffer_indices.clone()
    }

    /// Byte range inside the entry's page vertex buffer.
    pub fn vertices_buffer_range(&self) -> Range<wgpu::BufferAddress> {
        self.buffer_vertices.clone()
    }

    /// Byte range inside the entry's page layer-metadata buffer.
    pub fn layer_metadata_buffer_range(&self) -> Range<wgpu::BufferAddress> {
        self.buffer_layer_metadata.clone()
    }

    /// Byte range inside the entry's page feature-metadata buffer.
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

    /// Highest zoom level among all resident allocations in `O(1)`.
    #[inline]
    pub fn max_zoom(&self) -> Option<u8> {
        self.pyramid.max_zoom()
    }

    /// Iterates per-tile layer queues across the index in coarse-to-fine Morton order.
    pub fn iter(&self) -> impl Iterator<Item = impl Iterator<Item = &IndexEntry>> + '_ {
        self.pyramid.values().map(|entry| entry.layers.iter())
    }

    pub(super) fn pop_front(&mut self) -> Option<IndexEntry> {
        let (alloc_id, coords) = self.order.pop_first()?;
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
        id: &str,
        keep: Option<u64>,
    ) -> Vec<IndexEntry> {
        let Some(entry) = self.pyramid.get_mut(coords) else {
            return Vec::new();
        };
        let mut removed = Vec::new();
        let mut i = 0;
        while i < entry.layers.len() {
            let should_remove =
                entry.layers[i].style_layer.id == id && keep != Some(entry.layers[i].allocation_id);
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
