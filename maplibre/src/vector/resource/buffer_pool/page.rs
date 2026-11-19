//! Independent GPU buffer slabs (`VertexSlab` and `BufferSlab`) for [`BufferPool`](super::BufferPool).

use std::{fmt, ops::Range};

use super::{
    allocator::{lcm_u64, lockstep_vertex_alignment, RangeAllocator},
    BackingBuffer, BackingBufferType,
};
use crate::render::resource::BackingBufferDescriptor;

/// Sentinel page index for an unused slab family (e.g. `0`-byte `EmptyLayerMetadata` or `0`-byte uniform FM).
pub(super) const NO_SLAB: u16 = u16::MAX;

/// Steady-state vertices per on-demand GPU vertex slab (`32 Ki` on WebGPU, `128 Ki` on native).
#[cfg(target_arch = "wasm32")]
pub(super) const DEFAULT_PAGE_VERTICES: u64 = 32 * 1024;
/// Steady-state vertices per on-demand GPU vertex slab (`32 Ki` on WebGPU, `128 Ki` on native).
#[cfg(not(target_arch = "wasm32"))]
pub(super) const DEFAULT_PAGE_VERTICES: u64 = 128 * 1024;

/// Steady-state indices per on-demand GPU index slab (`96 Ki` on WebGPU, `256 Ki` on native).
#[cfg(target_arch = "wasm32")]
pub(super) const DEFAULT_PAGE_INDICES: u64 = 96 * 1024;
/// Steady-state indices per on-demand GPU index slab (`96 Ki` on WebGPU, `256 Ki` on native).
#[cfg(not(target_arch = "wasm32"))]
pub(super) const DEFAULT_PAGE_INDICES: u64 = 256 * 1024;

/// Steady-state records per on-demand GPU layer-metadata slab (`1,024` records = `64 KiB` at `64 B` stride).
pub(super) const DEFAULT_PAGE_LAYER_META: u64 = 1024;

/// Steady-state records per standalone feature-metadata slab (`32 Ki` on WebGPU, `64 Ki` on native).
#[cfg(target_arch = "wasm32")]
pub(super) const DEFAULT_PAGE_FEATURE_META: u64 = 32 * 1024;
/// Steady-state records per standalone feature-metadata slab (`32 Ki` on WebGPU, `64 Ki` on native).
#[cfg(not(target_arch = "wasm32"))]
pub(super) const DEFAULT_PAGE_FEATURE_META: u64 = 64 * 1024;

/// Where an [`IndexEntry`](super::IndexEntry)'s `feature_metadata` bytes reside.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FmStorageKind {
    /// Zero feature-metadata bytes reserved (uniform paint carried in `ShaderLayerMetadata`).
    None,
    /// Element-aligned 1:1 with `VertexSlab::vertices` inside `VertexSlab::paired_fm` (bound whole with `base_vertex`).
    Paired,
    /// Suballocated independently inside `feature_slabs` (`1`-record uniform FM or `2*V` dual circle stroke).
    Standalone,
}

/// GPU byte strides and alignment invariants for `(V::Gpu, I, TM, FM)`.
#[derive(Clone, Copy, Debug)]
pub(super) struct ElementStrides {
    pub vertex: u64,
    pub index: u64,
    pub layer_metadata: u64,
    pub feature_metadata: u64,
    pub paired_vertex_align: u64,
    pub uniform_vertex_align: u64,
    pub index_align: u64,
    pub u16_index_align: u64,
    pub layer_align: u64,
    pub feature_align: u64,
}

impl ElementStrides {
    pub const fn of<VGpu, I, TM, FM>() -> Self {
        let vertex = std::mem::size_of::<VGpu>() as u64;
        let index = std::mem::size_of::<I>() as u64;
        let layer_metadata = std::mem::size_of::<TM>() as u64;
        let feature_metadata = std::mem::size_of::<FM>() as u64;
        Self {
            vertex,
            index,
            layer_metadata,
            feature_metadata,
            paired_vertex_align: lockstep_vertex_alignment(vertex, feature_metadata),
            uniform_vertex_align: if vertex == 0 {
                wgpu::COPY_BUFFER_ALIGNMENT
            } else {
                lcm_u64(vertex, wgpu::COPY_BUFFER_ALIGNMENT)
            },
            index_align: if index == 0 {
                wgpu::COPY_BUFFER_ALIGNMENT
            } else {
                lcm_u64(index, wgpu::COPY_BUFFER_ALIGNMENT)
            },
            u16_index_align: wgpu::COPY_BUFFER_ALIGNMENT,
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
    pub const fn vertex_byte_align(self, paired_fm: bool, elem_align: u64) -> u64 {
        let base = if paired_fm {
            self.paired_vertex_align
        } else {
            self.uniform_vertex_align
        };
        if elem_align <= 1 || self.vertex == 0 {
            base
        } else {
            lcm_u64(base, elem_align * self.vertex)
        }
    }

    #[inline]
    pub const fn active_index_stride(self, use_u16: bool) -> u64 {
        if use_u16 {
            2
        } else {
            self.index
        }
    }

    #[inline]
    pub const fn active_index_align(self, use_u16: bool) -> u64 {
        if use_u16 {
            self.u16_index_align
        } else {
            self.index_align
        }
    }
}

/// Allocates a backing buffer of type `B` on demand when a slab family grows.
pub(super) struct PageFactory<B>(
    pub(super) Box<dyn Fn(BackingBufferType, wgpu::BufferAddress) -> B>,
);

impl<B> fmt::Debug for PageFactory<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PageFactory(..)")
    }
}

/// Releases GPU device storage owned by an individual backing buffer `B` when reclaimed or dropped.
pub(super) struct SlabDestroyer<B>(pub(super) fn(&B));

impl<B> fmt::Debug for SlabDestroyer<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SlabDestroyer(..)")
    }
}

/// Standalone homogeneous slab buffer (`u16` indices, `u32` indices, `layer_metadata`, or decoupled `feature_metadata`).
#[derive(Debug)]
pub(super) struct BufferSlab<B> {
    pub buffer: BackingBuffer<B>,
    pub alloc: RangeAllocator,
    pub element_capacity: u64,
    pub live_entries: u32,
    pub dedicated_oversize: bool,
}

impl<B> BufferSlab<B> {
    pub(super) fn new(desc: BackingBufferDescriptor<B>, elem_stride: u64) -> Self {
        let element_capacity = desc.inner_size.checked_div(elem_stride).unwrap_or(0);
        let usable_bytes = element_capacity * elem_stride;
        Self {
            buffer: BackingBuffer::new(desc.buffer, desc.inner_size),
            alloc: RangeAllocator::new(usable_bytes),
            element_capacity,
            live_entries: 0,
            dedicated_oversize: false,
        }
    }

    #[inline]
    pub(super) fn is_empty(&self) -> bool {
        self.live_entries == 0
    }

    pub(super) fn clear(&mut self) {
        self.alloc.clear();
        self.live_entries = 0;
    }
}

/// Vertex slab holding `V::Gpu` records and an optional 1:1 element-paired `FM` buffer.
///
/// Uniform layers suballocate from `VertexSlab`s with `paired_fm == None` (`0 B` feature metadata
/// overhead), while 1:1 per-vertex data-driven layers and symbols suballocate from `VertexSlab`s
/// with `paired_fm == Some(..)` so both Slot 0 and `feature_slot` bind whole (`slice(..)`) with `base_vertex`.
#[derive(Debug)]
pub(super) struct VertexSlab<B> {
    pub vertices: BackingBuffer<B>,
    pub paired_fm: Option<BackingBuffer<B>>,
    pub vertex_alloc: RangeAllocator,
    pub element_capacity: u64,
    pub live_entries: u32,
    pub paired_fm_dirty_end: u64,
    pub dedicated_oversize: bool,
}

impl<B> VertexSlab<B> {
    pub(super) fn new(
        vertices: BackingBufferDescriptor<B>,
        paired_fm: Option<BackingBufferDescriptor<B>>,
        strides: ElementStrides,
    ) -> Self {
        let v_cap = vertices.inner_size.checked_div(strides.vertex).unwrap_or(0);
        let element_capacity = match &paired_fm {
            Some(fm) => match fm.inner_size.checked_div(strides.feature_metadata) {
                Some(fm_cap) => v_cap.min(fm_cap),
                None => v_cap,
            },
            None => v_cap,
        };
        let usable_vertex_bytes = element_capacity * strides.vertex;
        Self {
            vertices: BackingBuffer::new(vertices.buffer, vertices.inner_size),
            paired_fm: paired_fm.map(|fm| BackingBuffer::new(fm.buffer, fm.inner_size)),
            vertex_alloc: RangeAllocator::new(usable_vertex_bytes),
            element_capacity,
            live_entries: 0,
            paired_fm_dirty_end: 0,
            dedicated_oversize: false,
        }
    }

    #[inline]
    pub(super) fn is_empty(&self) -> bool {
        self.live_entries == 0
    }

    pub(super) fn clear(&mut self) {
        self.vertex_alloc.clear();
        self.live_entries = 0;
    }
}

/// Input byte/alignment requirements for a single layer reservation across the pool's slab classes.
#[derive(Clone, Copy, Debug)]
pub(super) struct SlabRequest {
    pub vertex_bytes: u64,
    pub vertex_elem_align: u64,
    pub index_bytes: u64,
    pub index_u16: bool,
    pub layer_bytes: u64,
    pub feature_bytes: u64,
    pub fm_kind: FmStorageKind,
}

/// Slab indices and byte sub-ranges reserved atomically across the pool's slab vectors.
#[derive(Clone, Debug)]
pub(super) struct SlabAllocation {
    pub vertex_page: u16,
    pub index_page: u16,
    pub layer_page: u16,
    pub feature_page: u16,
    pub index_u16: bool,
    pub fm_kind: FmStorageKind,
    pub vertices: Range<u64>,
    pub indices: Range<u64>,
    pub layer_metadata: Range<u64>,
    pub feature_metadata: Range<u64>,
}
