use super::*;
#[derive(Debug, Clone)]
pub struct IndexEntry {
    pub(super) allocation_id: u64,
    pub coords: WorldTileCoords, // TODO: replace with generic key
    pub style_layer: StyleLayer, // TODO: remove
    // Range of bytes within the backing buffer for vertices
    pub(super) buffer_vertices: Range<wgpu::BufferAddress>,
    // Range of bytes within the backing buffer for indices
    pub(super) buffer_indices: Range<wgpu::BufferAddress>,
    // Range of bytes within the backing buffer for metadata
    pub(super) buffer_layer_metadata: Range<wgpu::BufferAddress>,
    // Range of bytes within the backing buffer for feature metadata
    pub(super) buffer_feature_metadata: Range<wgpu::BufferAddress>,
    // Amount of actually usable indices. Each index has the size/format `IndexDataType`.
    // Can be lower than size(buffer_indices) / indices_stride because of alignment.
    pub(super) usable_indices: u32,
}

impl IndexEntry {
    /// Identifies this upload, even when an evicted layer reuses the same buffer range.
    pub fn allocation_id(&self) -> u64 {
        self.allocation_id
    }

    pub fn indices_range(&self) -> Range<u32> {
        0..self.usable_indices
    }

    pub fn indices_buffer_range(&self) -> Range<wgpu::BufferAddress> {
        self.buffer_indices.clone()
    }

    pub fn vertices_buffer_range(&self) -> Range<wgpu::BufferAddress> {
        self.buffer_vertices.clone()
    }

    pub fn layer_metadata_buffer_range(&self) -> Range<wgpu::BufferAddress> {
        self.buffer_layer_metadata.clone()
    }

    pub fn feature_metadata_buffer_range(&self) -> Range<wgpu::BufferAddress> {
        self.buffer_feature_metadata.clone()
    }
}

#[derive(Debug)]
pub struct RingIndexEntry {
    layers: VecDeque<IndexEntry>,
}

#[derive(Debug)]
pub struct RingIndex {
    tree_index: BTreeMap<Quadkey, RingIndexEntry>,
    linear_index: VecDeque<Quadkey>,
}

impl RingIndex {
    pub fn new() -> Self {
        Self {
            tree_index: Default::default(),
            linear_index: Default::default(),
        }
    }

    pub fn clear(&mut self) {
        self.linear_index.clear();
        self.tree_index.clear();
    }

    pub fn front(&self) -> Option<&IndexEntry> {
        self.linear_index.front().and_then(|key| {
            self.tree_index
                .get(key)
                .and_then(|entry| entry.layers.front())
        })
    }

    pub fn back(&self) -> Option<&IndexEntry> {
        self.linear_index.back().and_then(|key| {
            self.tree_index
                .get(key)
                .and_then(|entry| entry.layers.back())
        })
    }

    pub fn get_layers(&self, coords: WorldTileCoords) -> Option<&VecDeque<IndexEntry>> {
        coords
            .build_quad_key()
            .and_then(|key| self.tree_index.get(&key))
            .map(|entry| &entry.layers)
    }

    pub fn iter(&self) -> impl Iterator<Item = impl Iterator<Item = &IndexEntry>> + '_ {
        self.linear_index
            .iter()
            .flat_map(|key| self.tree_index.get(key).map(|entry| entry.layers.iter()))
    }

    fn pop_front(&mut self) -> Option<IndexEntry> {
        let key = self.linear_index.pop_front()?;
        let entry = self.tree_index.get_mut(&key)?;
        let removed = entry.layers.pop_front();
        if entry.layers.is_empty() {
            self.tree_index.remove(&key);
        }
        removed
    }

    pub(super) fn remove_tile(&mut self, coords: WorldTileCoords) -> bool {
        let Some(key) = coords.build_quad_key() else {
            return false;
        };
        let removed = self.tree_index.remove(&key).is_some();
        self.linear_index.retain(|current| *current != key);
        removed
    }

    pub(super) fn push_back(&mut self, entry: IndexEntry) {
        if let Some(key) = entry.coords.build_quad_key() {
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

            self.linear_index.push_back(key)
        }
    }

    pub(super) fn make_room(
        &mut self,
        bytes: u64,
        typ: BackingBufferType,
        size: u64,
    ) -> Option<Range<u64>> {
        if bytes > size {
            return None;
        }
        loop {
            let gap = self.find_largest_gap(typ, size);
            if bytes <= gap.end - gap.start {
                return Some(gap.start..gap.start + bytes);
            }
            self.pop_front()?;
        }
    }

    pub(super) fn find_largest_gap(
        &self,
        typ: BackingBufferType,
        inner_size: wgpu::BufferAddress,
    ) -> Range<wgpu::BufferAddress> {
        let start = self.front().map(|first| match typ {
            BackingBufferType::Vertices => first.buffer_vertices.start,
            BackingBufferType::Indices => first.buffer_indices.start,
            BackingBufferType::Metadata => first.buffer_layer_metadata.start,
            BackingBufferType::FeatureMetadata => first.buffer_feature_metadata.start,
        });
        let end = self.back().map(|first| match typ {
            BackingBufferType::Vertices => first.buffer_vertices.end,
            BackingBufferType::Indices => first.buffer_indices.end,
            BackingBufferType::Metadata => first.buffer_layer_metadata.end,
            BackingBufferType::FeatureMetadata => first.buffer_feature_metadata.end,
        });

        if let Some(start) = start {
            if let Some(end) = end {
                if end > start {
                    // we haven't wrapped yet in the ring buffer

                    let gap_from_start = 0..start; // gap from beginning to first entry
                    let gap_to_end = end..inner_size;

                    if gap_to_end.end - gap_to_end.start > gap_from_start.end - gap_from_start.start
                    {
                        gap_to_end
                    } else {
                        gap_from_start
                    }
                } else {
                    // we already wrapped in the ring buffer
                    // we choose the gab between the two
                    end..start
                }
            } else {
                0..inner_size
            }
        } else {
            0..inner_size
        }
    }
}

impl Default for RingIndex {
    fn default() -> Self {
        Self::new()
    }
}
