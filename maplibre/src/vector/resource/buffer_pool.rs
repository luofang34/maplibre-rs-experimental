//! A ring-buffer like pool of [buffers](wgpu::Buffer).

use std::{
    collections::{btree_map, BTreeMap, HashSet, VecDeque},
    fmt::Debug,
    marker::PhantomData,
    mem::size_of,
    ops::Range,
};

use bytemuck::Pod;

use crate::{
    coords::{Quadkey, WorldTileCoords},
    render::settings::BufferPoolSizes,
    render::{
        resource::{BackingBufferDescriptor, Queue},
        tile_view_pattern::HasTile,
    },
    style::layer::StyleLayer,
    tcs::world::World,
    vector::tessellation::OverAlignedVertexBuffer,
};

/// This is inspired by the memory pool in Vulkan documented
/// [here](https://gpuopen-librariesandsdks.github.io/VulkanMemoryAllocator/html/custom_memory_pools.html).
#[derive(Debug)]
pub struct BufferPool<Q, B, V, I, TM, FM> {
    vertices: BackingBuffer<B>,
    indices: BackingBuffer<B>,
    layer_metadata: BackingBuffer<B>,
    feature_metadata: BackingBuffer<B>,

    index: RingIndex,
    /// Advances whenever geometry is allocated or evicted, so cached renders can refresh.
    revision: u64,
    phantom_v: PhantomData<V>,
    phantom_i: PhantomData<I>,
    phantom_q: PhantomData<Q>,
    phantom_m: PhantomData<TM>,
    phantom_fm: PhantomData<FM>,
}

#[derive(Clone, Copy, Debug)]
pub enum BackingBufferType {
    Vertices,
    Indices,
    Metadata,
    FeatureMetadata,
}

#[derive(Debug)]
struct BackingBuffer<B> {
    /// The internal structure which is used for storage
    inner: B,
    /// The size of the `inner` buffer
    inner_size: wgpu::BufferAddress,
    typ: BackingBufferType,
}

impl<B> BackingBuffer<B> {
    fn new(inner: B, inner_size: wgpu::BufferAddress, typ: BackingBufferType) -> Self {
        Self {
            inner,
            inner_size,
            typ,
        }
    }
}

/// Bytes for `count` elements of `element` bytes, cut down to whole elements of the largest
/// buffer the device creates; the pool is sized for desktop GPUs and a simulator allows less.
fn fitting_buffer_size(
    element: usize,
    count: wgpu::BufferAddress,
    largest: wgpu::BufferAddress,
) -> wgpu::BufferAddress {
    let element = element as wgpu::BufferAddress;
    (element * count).min(largest) / element * element
}

impl<Q: Queue<B>, B, V: Pod, I: Pod, TM: Pod, FM: Pod> BufferPool<Q, B, V, I, TM, FM> {
    pub fn new(
        vertices: BackingBufferDescriptor<B>,
        indices: BackingBufferDescriptor<B>,
        layer_metadata: BackingBufferDescriptor<B>,
        feature_metadata: BackingBufferDescriptor<B>,
    ) -> Self {
        Self {
            vertices: BackingBuffer::new(
                vertices.buffer,
                vertices.inner_size,
                BackingBufferType::Vertices,
            ),
            indices: BackingBuffer::new(
                indices.buffer,
                indices.inner_size,
                BackingBufferType::Indices,
            ),
            layer_metadata: BackingBuffer::new(
                layer_metadata.buffer,
                layer_metadata.inner_size,
                BackingBufferType::Metadata,
            ),
            feature_metadata: BackingBuffer::new(
                feature_metadata.buffer,
                feature_metadata.inner_size,
                BackingBufferType::FeatureMetadata,
            ),
            index: RingIndex::new(),
            revision: 0,
            phantom_v: Default::default(),
            phantom_i: Default::default(),
            phantom_q: Default::default(),
            phantom_m: Default::default(),
            phantom_fm: Default::default(),
        }
    }

    pub fn clear(&mut self) {
        self.index.clear();
        self.revision = self.revision.wrapping_add(1);
    }

    /// Releases GPU geometry when the tile data and its glyph atlas leave the CPU cache.
    pub fn remove_tile(&mut self, coords: WorldTileCoords) {
        if self.index.remove_tile(coords) {
            self.revision = self.revision.wrapping_add(1);
        }
    }

    #[cfg(test)]
    fn available_space(&self, typ: BackingBufferType) -> wgpu::BufferAddress {
        let gap = self.index.find_largest_gap(
            typ,
            match typ {
                BackingBufferType::Vertices => &self.vertices,
                BackingBufferType::Indices => &self.indices,
                BackingBufferType::Metadata => &self.layer_metadata,
                BackingBufferType::FeatureMetadata => &self.feature_metadata,
            }
            .inner_size,
        );

        gap.end - gap.start
    }

    pub fn vertices(&self) -> &B {
        &self.vertices.inner
    }

    pub fn indices(&self) -> &B {
        &self.indices.inner
    }

    pub fn metadata(&self) -> &B {
        &self.layer_metadata.inner
    }

    pub fn feature_metadata(&self) -> &B {
        &self.feature_metadata.inner
    }

    /// The VertexBuffers can contain padding elements. Not everything from a VertexBuffers is usable.
    /// The function returns the `bytes` and `aligned_bytes`. See [`OverAlignedVertexBuffer`].
    fn align(
        stride: wgpu::BufferAddress,
        elements: wgpu::BufferAddress,
        usable_elements: wgpu::BufferAddress,
    ) -> (wgpu::BufferAddress, wgpu::BufferAddress) {
        let bytes = elements * stride;

        let usable_bytes = (usable_elements * stride) as wgpu::BufferAddress;

        let align = wgpu::COPY_BUFFER_ALIGNMENT;
        let padding = (align - usable_bytes % align) % align;

        let aligned_bytes = usable_bytes + padding;

        (bytes, aligned_bytes)
    }

    /// Number of allocations so far; changes whenever the geometry held changes.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn get_loaded_style_layers_at(&self, coords: WorldTileCoords) -> Option<HashSet<&str>> {
        self.index.get_layers(coords).map(|layers| {
            layers
                .iter()
                .map(|entry| entry.style_layer.id.as_str())
                .collect()
        })
    }

    #[tracing::instrument(skip_all)]
    pub fn update_layer_metadata(&self, queue: &Q, entry: &IndexEntry, layer_metadata: TM) {
        let layer_metadata_stride = size_of::<TM>() as wgpu::BufferAddress; // TODO: deduplicate
        let (layer_metadata_bytes, aligned_layer_metadata_bytes) =
            Self::align(layer_metadata_stride, 1, 1);

        let allocated = entry.buffer_layer_metadata.end - entry.buffer_layer_metadata.start;
        if allocated != layer_metadata_bytes {
            tracing::error!(
                coords = %entry.coords,
                layer = %entry.style_layer.id,
                allocated,
                offered = layer_metadata_bytes,
                "layer metadata update skipped: size differs from the allocation"
            );
            return;
        }

        queue.write_buffer(
            &self.layer_metadata.inner,
            entry.buffer_layer_metadata.start,
            &bytemuck::cast_slice(&[layer_metadata])[0..aligned_layer_metadata_bytes as usize],
        );
    }

    #[tracing::instrument(skip_all)]
    pub fn update_feature_metadata(&self, queue: &Q, entry: &IndexEntry, feature_metadata: &[FM]) {
        let feature_metadata_stride = size_of::<FM>() as wgpu::BufferAddress; // TODO: deduplicate

        let (feature_metadata_bytes, aligned_feature_metadata_bytes) = Self::align(
            feature_metadata_stride,
            feature_metadata.len() as wgpu::BufferAddress,
            feature_metadata.len() as wgpu::BufferAddress,
        );

        let allocated = entry.buffer_feature_metadata.end - entry.buffer_feature_metadata.start;
        if allocated != feature_metadata_bytes
            || feature_metadata_bytes != aligned_feature_metadata_bytes
        {
            // Writing past the allocation would corrupt a neighbouring layer's metadata.
            tracing::error!(
                coords = %entry.coords,
                layer = %entry.style_layer.id,
                allocated,
                offered = feature_metadata_bytes,
                aligned = aligned_feature_metadata_bytes,
                "feature metadata update skipped: size differs from the allocation"
            );
            return;
        }

        queue.write_buffer(
            &self.feature_metadata.inner,
            entry.buffer_feature_metadata.start,
            &bytemuck::cast_slice(feature_metadata)[0..aligned_feature_metadata_bytes as usize],
        );
    }

    pub fn index(&self) -> &RingIndex {
        &self.index
    }
}

impl<Q: Queue<B>, B, V: Pod, I: Pod, TM: Pod, FM: Pod> HasTile for BufferPool<Q, B, V, I, TM, FM> {
    fn has_tile(&self, coords: WorldTileCoords, _world: &World) -> bool {
        self.index().get_layers(coords).is_some()
    }
}

mod device;
mod index;
mod upload;
pub use index::{IndexEntry, RingIndex};
mod allocation_error;
pub use allocation_error::AllocationError;
#[cfg(test)]
mod tests;
