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

/// Uploaded layer entries belonging to a single tile quadkey.
#[derive(Debug)]
pub struct RingIndexEntry {
    layers: VecDeque<IndexEntry>,
}

/// Spatial (`Quadkey`) and temporal (`allocation_id`) index of resident geometry allocations.
#[derive(Debug)]
pub struct RingIndex {
    tree_index: BTreeMap<Quadkey, RingIndexEntry>,
    order: BTreeMap<u64, Quadkey>,
    zoom_counts: [u32; 32],
}

impl RingIndex {
    /// Creates an empty index.
    pub fn new() -> Self {
        Self {
            tree_index: BTreeMap::new(),
            order: BTreeMap::new(),
            zoom_counts: [0; 32],
        }
    }

    /// Removes all indexed entries in `O(1)` drop without per-entry rebalancing.
    pub fn clear(&mut self) {
        self.tree_index.clear();
        self.order.clear();
        self.zoom_counts = [0; 32];
    }

    /// Oldest resident allocation in FIFO upload order.
    pub fn front(&self) -> Option<&IndexEntry> {
        let (&alloc_id, key) = self.order.first_key_value()?;
        self.tree_index
            .get(key)?
            .layers
            .iter()
            .find(|entry| entry.allocation_id == alloc_id)
    }

    /// Newest resident allocation in FIFO upload order.
    pub fn back(&self) -> Option<&IndexEntry> {
        let (&alloc_id, key) = self.order.last_key_value()?;
        self.tree_index
            .get(key)?
            .layers
            .iter()
            .rev()
            .find(|entry| entry.allocation_id == alloc_id)
    }

    /// Resident layer allocations at `coords`, if any.
    pub fn get_layers(&self, coords: WorldTileCoords) -> Option<&VecDeque<IndexEntry>> {
        coords
            .build_quad_key()
            .and_then(|key| self.tree_index.get(&key))
            .map(|entry| &entry.layers)
    }

    /// Highest zoom level among all resident allocations in `O(1)`.
    pub fn max_zoom(&self) -> Option<u8> {
        self.zoom_counts
            .iter()
            .rposition(|&count| count > 0)
            .map(|z| z as u8)
    }

    /// Iterates per-tile layer queues across the index.
    pub fn iter(&self) -> impl Iterator<Item = impl Iterator<Item = &IndexEntry>> + '_ {
        self.tree_index.values().map(|entry| entry.layers.iter())
    }

    pub(super) fn pop_front(&mut self) -> Option<IndexEntry> {
        let (alloc_id, key) = self.order.pop_first()?;
        let entry = self.tree_index.get_mut(&key)?;
        let pos = entry
            .layers
            .iter()
            .position(|layer| layer.allocation_id == alloc_id)?;
        let removed = entry.layers.remove(pos)?;
        dec_zoom(&mut self.zoom_counts, removed.coords);
        if entry.layers.is_empty() {
            self.tree_index.remove(&key);
        }
        Some(removed)
    }

    pub(super) fn remove_tile(&mut self, coords: WorldTileCoords) -> Vec<IndexEntry> {
        let Some(key) = coords.build_quad_key() else {
            return Vec::new();
        };
        let Some(entry) = self.tree_index.remove(&key) else {
            return Vec::new();
        };
        let mut removed = Vec::with_capacity(entry.layers.len());
        for layer in entry.layers {
            self.order.remove(&layer.allocation_id);
            dec_zoom(&mut self.zoom_counts, layer.coords);
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
        let Some(key) = coords.build_quad_key() else {
            return Vec::new();
        };
        let Some(entry) = self.tree_index.get_mut(&key) else {
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
                    dec_zoom(&mut self.zoom_counts, layer.coords);
                    removed.push(layer);
                }
            } else {
                i += 1;
            }
        }
        if entry.layers.is_empty() {
            self.tree_index.remove(&key);
        }
        removed
    }

    pub(super) fn push_back(&mut self, entry: IndexEntry) {
        if let Some(key) = entry.coords.build_quad_key() {
            self.order.insert(entry.allocation_id, key);
            inc_zoom(&mut self.zoom_counts, entry.coords);
            match self.tree_index.entry(key) {
                btree_map::Entry::Vacant(index_entry) => {
                    index_entry.insert(RingIndexEntry {
                        layers: VecDeque::from([entry]),
                    });
                }
                btree_map::Entry::Occupied(mut index_entry) => {
                    index_entry.get_mut().layers.push_back(entry);
                }
            }
        }
    }
}

#[inline]
fn inc_zoom(zoom_counts: &mut [u32; 32], coords: WorldTileCoords) {
    let z = usize::from(u8::from(coords.z)).min(31);
    zoom_counts[z] = zoom_counts[z].saturating_add(1);
}

#[inline]
fn dec_zoom(zoom_counts: &mut [u32; 32], coords: WorldTileCoords) {
    let z = usize::from(u8::from(coords.z)).min(31);
    zoom_counts[z] = zoom_counts[z].saturating_sub(1);
}

impl Default for RingIndex {
    fn default() -> Self {
        Self::new()
    }
}
