use super::*;

/// Minimal pass binding interface consumed by [`BufferPool::bind_layer`].
pub trait BufferBindPass {
    /// Binds `slice` at vertex buffer `slot`.
    fn set_vertex_buffer(&mut self, slot: u32, slice: wgpu::BufferSlice<'_>);
    /// Binds `slice` as the index buffer with `format`.
    fn set_index_buffer(&mut self, slice: wgpu::BufferSlice<'_>, format: wgpu::IndexFormat);
}

impl<V: PoolVertex, I: Pod, TM: Pod, FM: Pod>
    BufferPool<crate::render::upload_queue::UploadQueue, wgpu::Buffer, V, I, TM, FM>
{
    /// Creates an on-demand paged GPU pool with [`BufferPoolSizes::default`] maximum capacity.
    pub fn from_device(device: &wgpu::Device) -> Self {
        Self::from_device_with_sizes(device, BufferPoolSizes::default())
    }

    /// Creates an on-demand paged GPU pool whose total element capacities across slabs are bounded
    /// by `sizes`, with each buffer clipped to the device's `max_buffer_size`.
    ///
    /// Zero GPU buffers are allocated until the first non-empty geometry upload arrives, and
    /// unused buffer classes (`EmptyLayerMetadata`, standalone `feature_metadata`, `u32` fallback
    /// indices) remain at `0` bytes until genuinely required.
    pub fn from_device_with_sizes(device: &wgpu::Device, sizes: BufferPoolSizes) -> Self {
        let max_page_bytes = device.limits().max_buffer_size;
        let device = device.clone();
        let mut pool = Self::with_page_factory(sizes, max_page_bytes, move |typ, logical_bytes| {
            let physical = logical_bytes.max(16).next_multiple_of(16);
            let (label, usage) = match typ {
                BackingBufferType::Vertices => (
                    "vertex slab",
                    wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                ),
                BackingBufferType::Indices => (
                    "index slab",
                    wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                ),
                BackingBufferType::Metadata => (
                    "layer metadata slab",
                    wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                ),
                BackingBufferType::FeatureMetadata => (
                    "feature metadata slab",
                    wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                ),
            };
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: physical,
                usage,
                mapped_at_creation: false,
            })
        });
        pool.slab_destroyer = Some(page::SlabDestroyer(|buf| buf.destroy()));
        pool.enable_u16_indices = size_of::<I>() == size_of::<u32>();
        pool.enable_u16_chunks = false;
        pool.zero_initialized_slabs = true;
        pool
    }

    /// Enables or disables `u16` base-vertex index chunks (when `DownlevelFlags::BASE_VERTEX`
    /// and `u16` indices are both supported).
    pub fn with_u16_chunks(mut self, enabled: bool) -> Self {
        self.enable_u16_chunks = enabled && self.enable_u16_indices;
        self
    }

    /// Total GPU buffer bytes currently physically allocated on the device across all live slabs.
    pub fn device_buffer_bytes(&self) -> wgpu::BufferAddress {
        let v_bytes: u64 = self
            .vertex_slabs
            .iter()
            .flatten()
            .map(|slab| {
                slab.vertices.inner.size() + slab.paired_fm.as_ref().map_or(0, |fm| fm.inner.size())
            })
            .sum();
        let other_bytes: u64 = self
            .index_u16_slabs
            .iter()
            .flatten()
            .chain(self.index_native_slabs.iter().flatten())
            .chain(self.layer_slabs.iter().flatten())
            .chain(self.feature_slabs.iter().flatten())
            .map(|slab| slab.buffer.inner.size())
            .sum();
        v_bytes + other_bytes
    }

    /// Binds a layer's indices (`u16` or `u32`), vertices, and feature metadata (at `feature_slot`)
    /// from their decoupled slabs and returns `(first_index, base_vertex)`.
    pub fn bind_layer<P: BufferBindPass>(
        &self,
        pass: &mut P,
        entry: &IndexEntry,
        (feature_slot, whole): (u32, bool),
    ) -> (u32, i32) {
        let format = if entry.index_u16 {
            wgpu::IndexFormat::Uint16
        } else {
            crate::render::INDEX_FORMAT
        };
        let idx_buf = self.indices_for(entry);
        let v_slab = self.vertex_slab_of(entry.vertex_page);

        if entry.is_uniform_feature_metadata() {
            let fm_range = entry.feature_metadata_buffer_range();
            if !fm_range.is_empty() {
                pass.set_vertex_buffer(
                    feature_slot,
                    self.feature_metadata_for(entry).slice(fm_range),
                );
            }
            if whole {
                pass.set_index_buffer(idx_buf.slice(..), format);
                pass.set_vertex_buffer(0, v_slab.vertices.inner.slice(..));
                (entry.whole_buffer_indices().start, entry.base_vertex())
            } else {
                pass.set_index_buffer(idx_buf.slice(entry.indices_buffer_range()), format);
                pass.set_vertex_buffer(
                    0,
                    v_slab.vertices.inner.slice(entry.vertices_buffer_range()),
                );
                (0, 0)
            }
        } else if whole && entry.has_paired_fm() && !entry.is_dual_feature_metadata() {
            pass.set_index_buffer(idx_buf.slice(..), format);
            pass.set_vertex_buffer(0, v_slab.vertices.inner.slice(..));
            pass.set_vertex_buffer(feature_slot, self.feature_metadata_for(entry).slice(..));
            (entry.whole_buffer_indices().start, entry.base_vertex())
        } else if whole {
            pass.set_index_buffer(idx_buf.slice(..), format);
            pass.set_vertex_buffer(
                0,
                v_slab.vertices.inner.slice(entry.vertices_buffer_range()),
            );
            pass.set_vertex_buffer(
                feature_slot,
                self.feature_metadata_for(entry)
                    .slice(entry.buffer_feature_metadata.start..),
            );
            (entry.whole_buffer_indices().start, 0)
        } else {
            pass.set_index_buffer(idx_buf.slice(entry.indices_buffer_range()), format);
            pass.set_vertex_buffer(
                0,
                v_slab.vertices.inner.slice(entry.vertices_buffer_range()),
            );
            pass.set_vertex_buffer(
                feature_slot,
                self.feature_metadata_for(entry)
                    .slice(entry.buffer_feature_metadata.start..),
            );
            (0, 0)
        }
    }
}

impl<Q: Queue<B>, B, V: PoolVertex, I: Pod, TM: Pod, FM: Pod> HasTile
    for BufferPool<Q, B, V, I, TM, FM>
{
    fn has_tile(&self, coords: WorldTileCoords, _world: &World) -> bool {
        self.as_ref().get_layers(coords).is_some()
    }

    fn nearest_ancestor_candidate(
        &self,
        coords: WorldTileCoords,
        _world: &World,
    ) -> Option<WorldTileCoords> {
        self.as_ref().nearest_ancestor_coords(coords)
    }

    fn has_descendant_candidates(
        &self,
        coords: WorldTileCoords,
        max_depth: usize,
        _world: &World,
    ) -> bool {
        self.as_ref().has_descendants(coords, max_depth)
    }
}
