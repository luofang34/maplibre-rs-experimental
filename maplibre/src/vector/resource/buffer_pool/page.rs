//! Backing buffer page combining vertex, index, layer-metadata, and feature-metadata slabs.

use std::{fmt, ops::Range};

use super::{
    allocator::{lcm_u64, lockstep_vertex_alignment, RangeAllocator},
    BackingBuffer, BackingBufferType,
};
use crate::render::resource::BackingBufferDescriptor;

/// Steady-state vertices per on-demand GPU page (`256 Ki` after the initial `16 Ki..128 Ki` ramp).
/// Index and metadata capacities per page scale proportionally with remaining [`BufferPoolSizes`](crate::render::settings::BufferPoolSizes).
pub(super) const DEFAULT_PAGE_VERTICES: u64 = 256 * 1024;

/// Byte capacities across the four buffer families of a pool or page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct ByteCapacities {
    pub vertices: u64,
    pub indices: u64,
    pub layer_metadata: u64,
    pub feature_metadata: u64,
}

impl ByteCapacities {
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.vertices == 0
            && self.indices == 0
            && self.layer_metadata == 0
            && self.feature_metadata == 0
    }

    #[cfg(test)]
    #[inline]
    pub const fn get(&self, typ: BackingBufferType) -> u64 {
        match typ {
            BackingBufferType::Vertices => self.vertices,
            BackingBufferType::Indices => self.indices,
            BackingBufferType::Metadata => self.layer_metadata,
            BackingBufferType::FeatureMetadata => self.feature_metadata,
        }
    }

    #[inline]
    pub const fn saturating_sub(self, rhs: Self) -> Self {
        Self {
            vertices: self.vertices.saturating_sub(rhs.vertices),
            indices: self.indices.saturating_sub(rhs.indices),
            layer_metadata: self.layer_metadata.saturating_sub(rhs.layer_metadata),
            feature_metadata: self.feature_metadata.saturating_sub(rhs.feature_metadata),
        }
    }

    #[inline]
    pub const fn saturating_add(self, rhs: Self) -> Self {
        Self {
            vertices: self.vertices.saturating_add(rhs.vertices),
            indices: self.indices.saturating_add(rhs.indices),
            layer_metadata: self.layer_metadata.saturating_add(rhs.layer_metadata),
            feature_metadata: self.feature_metadata.saturating_add(rhs.feature_metadata),
        }
    }
}

/// Stride configuration for the four typed sub-buffers of a [`BufferPool`](super::BufferPool).
#[derive(Clone, Copy, Debug)]
pub(super) struct ElementStrides {
    pub vertex: u64,
    pub index: u64,
    pub layer_metadata: u64,
    pub feature_metadata: u64,
    pub vertex_align: u64,
    pub decoupled_vertex_align: u64,
    pub index_align: u64,
    pub layer_align: u64,
    pub feature_align: u64,
}

impl ElementStrides {
    pub const fn of<V, I, TM, FM>() -> Self {
        let vertex = std::mem::size_of::<V>() as u64;
        let index = std::mem::size_of::<I>() as u64;
        let layer_metadata = std::mem::size_of::<TM>() as u64;
        let feature_metadata = std::mem::size_of::<FM>() as u64;
        Self {
            vertex,
            index,
            layer_metadata,
            feature_metadata,
            vertex_align: lockstep_vertex_alignment(vertex, feature_metadata),
            decoupled_vertex_align: if vertex == 0 {
                wgpu::COPY_BUFFER_ALIGNMENT
            } else {
                lcm_u64(vertex, wgpu::COPY_BUFFER_ALIGNMENT)
            },
            index_align: if index == 0 {
                wgpu::COPY_BUFFER_ALIGNMENT
            } else {
                lcm_u64(index, wgpu::COPY_BUFFER_ALIGNMENT)
            },
            layer_align: if layer_metadata == 0 {
                wgpu::COPY_BUFFER_ALIGNMENT
            } else {
                lcm_u64(layer_metadata, wgpu::COPY_BUFFER_ALIGNMENT)
            },
            feature_align: if feature_metadata == 0 {
                wgpu::COPY_BUFFER_ALIGNMENT
            } else {
                lcm_u64(feature_metadata, wgpu::COPY_BUFFER_ALIGNMENT)
            },
        }
    }

    #[inline]
    pub const fn effective_vertex_align(self, decoupled: bool) -> u64 {
        if decoupled {
            self.decoupled_vertex_align
        } else {
            self.vertex_align
        }
    }
}

/// Allocates a backing buffer of type `B` on demand when a paged pool grows.
pub(super) struct PageFactory<B>(
    pub(super) Box<dyn Fn(BackingBufferType, wgpu::BufferAddress) -> B>,
);

impl<B> fmt::Debug for PageFactory<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PageFactory(..)")
    }
}

/// Releases GPU device storage owned by a [`BufferPage`] when reclaimed or dropped.
pub(super) struct PageDestroyer<B>(pub(super) fn(&BufferPage<B>));

impl<B> fmt::Debug for PageDestroyer<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PageDestroyer(..)")
    }
}

/// One self-contained slab page of vertex, index, layer-metadata, and feature-metadata buffers.
#[derive(Debug)]
pub(super) struct BufferPage<B> {
    pub vertices: BackingBuffer<B>,
    pub indices: BackingBuffer<B>,
    pub layer_metadata: BackingBuffer<B>,
    pub feature_metadata: BackingBuffer<B>,
    pub vertex_alloc: RangeAllocator,
    pub index_alloc: RangeAllocator,
    pub layer_alloc: RangeAllocator,
    pub feature_alloc: RangeAllocator,
    pub decoupled_feature_metadata: bool,
    pub live_entries: u32,
}

/// Ranges reserved atomically on a single [`BufferPage`].
#[derive(Clone, Debug)]
pub(super) struct PageAllocation {
    pub page: u16,
    pub vertices: Range<u64>,
    pub indices: Range<u64>,
    pub layer_metadata: Range<u64>,
    pub feature_metadata: Range<u64>,
}

impl<B> BufferPage<B> {
    pub(super) fn new(
        vertices: BackingBufferDescriptor<B>,
        indices: BackingBufferDescriptor<B>,
        layer_metadata: BackingBufferDescriptor<B>,
        feature_metadata: BackingBufferDescriptor<B>,
        strides: ElementStrides,
        decoupled_feature_metadata: bool,
    ) -> Self {
        let v_cap = vertices.inner_size.checked_div(strides.vertex).unwrap_or(0);
        let vertex_elements = if decoupled_feature_metadata {
            v_cap
        } else {
            match feature_metadata
                .inner_size
                .checked_div(strides.feature_metadata)
            {
                Some(fm_cap) => v_cap.min(fm_cap),
                None => v_cap,
            }
        };
        let usable_vertex_bytes = vertex_elements * strides.vertex;
        let usable_index_bytes =
            indices.inner_size.checked_div(strides.index).unwrap_or(0) * strides.index;
        let usable_layer_bytes = layer_metadata
            .inner_size
            .checked_div(strides.layer_metadata)
            .unwrap_or(0)
            * strides.layer_metadata;
        let usable_feature_bytes = feature_metadata
            .inner_size
            .checked_div(strides.feature_metadata)
            .unwrap_or(0)
            * strides.feature_metadata;

        Self {
            vertices: BackingBuffer::new(vertices.buffer, vertices.inner_size),
            indices: BackingBuffer::new(indices.buffer, indices.inner_size),
            layer_metadata: BackingBuffer::new(layer_metadata.buffer, layer_metadata.inner_size),
            feature_metadata: BackingBuffer::new(
                feature_metadata.buffer,
                feature_metadata.inner_size,
            ),
            vertex_alloc: RangeAllocator::new(usable_vertex_bytes),
            index_alloc: RangeAllocator::new(usable_index_bytes),
            layer_alloc: RangeAllocator::new(usable_layer_bytes),
            feature_alloc: RangeAllocator::new(usable_feature_bytes),
            decoupled_feature_metadata,
            live_entries: 0,
        }
    }

    #[inline]
    pub(super) fn capacities(&self) -> ByteCapacities {
        ByteCapacities {
            vertices: self.vertices.inner_size,
            indices: self.indices.inner_size,
            layer_metadata: self.layer_metadata.inner_size,
            feature_metadata: self.feature_metadata.inner_size,
        }
    }

    #[inline]
    pub(super) fn is_empty(&self) -> bool {
        self.live_entries == 0
    }

    pub(super) fn clear(&mut self) {
        self.vertex_alloc.clear();
        self.index_alloc.clear();
        self.layer_alloc.clear();
        self.feature_alloc.clear();
        self.live_entries = 0;
    }

    pub(super) fn try_allocate(
        &mut self,
        page_id: u16,
        sizes: [u64; 4],
        strides: ElementStrides,
    ) -> Option<PageAllocation> {
        let v_align = strides.effective_vertex_align(self.decoupled_feature_metadata);
        if self.decoupled_feature_metadata {
            if !self
                .feature_alloc
                .can_allocate(sizes[3], strides.feature_align)
            {
                return None;
            }
        } else if sizes[3] > self.feature_metadata.inner_size {
            return None;
        }
        if !self.vertex_alloc.can_allocate(sizes[0], v_align)
            || !self.index_alloc.can_allocate(sizes[1], strides.index_align)
            || !self.layer_alloc.can_allocate(sizes[2], strides.layer_align)
        {
            return None;
        }
        let vertices = self.vertex_alloc.allocate(sizes[0], v_align)?;
        let Some(indices) = self.index_alloc.allocate(sizes[1], strides.index_align) else {
            self.vertex_alloc.free(vertices);
            return None;
        };
        let Some(layer_metadata) = self.layer_alloc.allocate(sizes[2], strides.layer_align) else {
            self.vertex_alloc.free(vertices);
            self.index_alloc.free(indices);
            return None;
        };
        let feature_metadata = if self.decoupled_feature_metadata {
            let Some(fm) = self.feature_alloc.allocate(sizes[3], strides.feature_align) else {
                self.vertex_alloc.free(vertices);
                self.index_alloc.free(indices);
                self.layer_alloc.free(layer_metadata);
                return None;
            };
            fm
        } else {
            let first_vertex = vertices.start.checked_div(strides.vertex).unwrap_or(0);
            let fm_start = first_vertex * strides.feature_metadata;
            fm_start..fm_start + sizes[3]
        };
        self.live_entries += 1;
        Some(PageAllocation {
            page: page_id,
            vertices,
            indices,
            layer_metadata,
            feature_metadata,
        })
    }

    pub(super) fn free(
        &mut self,
        vertices: Range<u64>,
        indices: Range<u64>,
        layer_metadata: Range<u64>,
        feature_metadata: Range<u64>,
        (release_vertices, release_layer_meta): (bool, bool),
    ) {
        if release_vertices {
            self.vertex_alloc.free(vertices);
        }
        self.index_alloc.free(indices);
        if release_layer_meta {
            self.layer_alloc.free(layer_metadata);
        }
        if self.decoupled_feature_metadata {
            self.feature_alloc.free(feature_metadata);
        }
        self.live_entries = self.live_entries.saturating_sub(1);
    }
}
