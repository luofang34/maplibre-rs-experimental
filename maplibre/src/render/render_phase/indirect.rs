//! Indirect draw argument structures and GPU parameter buffers for [`BinnedRenderPhase`](super::BinnedRenderPhase).

use std::{mem::size_of, ops::Range};

use bytemuck_derive::{Pod, Zeroable};

mod buffers;
pub use buffers::IndirectParametersBuffers;

/// GPU argument layout for non-indexed `draw_indirect` and `multi_draw_indirect` commands (16 bytes).
///
/// Matches the WebGPU / Vulkan `VkDrawIndirectCommand` binary layout and Bevy's
/// `IndirectParametersNonIndexed`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct DrawIndirectArgs {
    /// Number of vertices to draw per instance.
    pub vertex_count: u32,
    /// Number of instances to draw (`0` when culled by a GPU compute pass).
    pub instance_count: u32,
    /// Index of the first vertex read from bound vertex buffers.
    pub first_vertex: u32,
    /// Instance index of the first instance drawn (`0` unless `INDIRECT_FIRST_INSTANCE` is enabled).
    pub first_instance: u32,
}

impl DrawIndirectArgs {
    /// Byte stride between packed [`DrawIndirectArgs`] entries (`16` bytes).
    pub const STRIDE: wgpu::BufferAddress = size_of::<Self>() as wgpu::BufferAddress;

    /// Returns the raw byte representation of this indirect command.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        bytemuck::bytes_of(self)
    }
}

impl From<wgpu::util::DrawIndirectArgs> for DrawIndirectArgs {
    fn from(args: wgpu::util::DrawIndirectArgs) -> Self {
        Self {
            vertex_count: args.vertex_count,
            instance_count: args.instance_count,
            first_vertex: args.first_vertex,
            first_instance: args.first_instance,
        }
    }
}

impl From<DrawIndirectArgs> for wgpu::util::DrawIndirectArgs {
    fn from(args: DrawIndirectArgs) -> Self {
        Self {
            vertex_count: args.vertex_count,
            instance_count: args.instance_count,
            first_vertex: args.first_vertex,
            first_instance: args.first_instance,
        }
    }
}

/// GPU argument layout for indexed `draw_indexed_indirect` and `multi_draw_indexed_indirect` commands (20 bytes).
///
/// Matches the WebGPU / Vulkan `VkDrawIndexedIndirectCommand` binary layout and Bevy's
/// `IndirectParametersIndexed`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct DrawIndexedIndirectArgs {
    /// Number of indices to read per instance.
    pub index_count: u32,
    /// Number of instances to draw (`0` when culled by a GPU compute pass).
    pub instance_count: u32,
    /// Offset of the first index within the bound index buffer.
    pub first_index: u32,
    /// Signed offset added to each index before fetching from vertex buffers.
    pub base_vertex: i32,
    /// Instance index of the first instance drawn (`0` unless `INDIRECT_FIRST_INSTANCE` is enabled).
    pub first_instance: u32,
}

impl DrawIndexedIndirectArgs {
    /// Byte stride between packed [`DrawIndexedIndirectArgs`] entries (`20` bytes).
    pub const STRIDE: wgpu::BufferAddress = size_of::<Self>() as wgpu::BufferAddress;

    /// Returns the raw byte representation of this indexed indirect command.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        bytemuck::bytes_of(self)
    }
}

impl From<wgpu::util::DrawIndexedIndirectArgs> for DrawIndexedIndirectArgs {
    fn from(args: wgpu::util::DrawIndexedIndirectArgs) -> Self {
        Self {
            index_count: args.index_count,
            instance_count: args.instance_count,
            first_index: args.first_index,
            base_vertex: args.base_vertex,
            first_instance: args.first_instance,
        }
    }
}

impl From<DrawIndexedIndirectArgs> for wgpu::util::DrawIndexedIndirectArgs {
    fn from(args: DrawIndexedIndirectArgs) -> Self {
        Self {
            index_count: args.index_count,
            instance_count: args.instance_count,
            first_index: args.first_index,
            base_vertex: args.base_vertex,
            first_instance: args.first_instance,
        }
    }
}

/// GPU/CPU batch-set header tracking indirect command count and base slot (8 bytes).
///
/// Mirrors Bevy's `IndirectBatchSet`. Placing `indirect_parameters_count` at byte offset `0`
/// allows a GPU compute culling pass to atomically compact surviving draws and pass the same
/// buffer directly as `count_buffer` to `multi_draw_[indexed_]indirect_count`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct IndirectBatchSet {
    /// Number of indirect draw commands active in this batch set.
    pub indirect_parameters_count: u32,
    /// Starting slot index in the indexed or non-indexed indirect parameter buffer.
    pub indirect_parameters_base: u32,
}

impl IndirectBatchSet {
    /// Byte stride between packed [`IndirectBatchSet`] entries (`8` bytes).
    pub const STRIDE: wgpu::BufferAddress = size_of::<Self>() as wgpu::BufferAddress;
}

/// CPU-initialized blueprint mapping an output instance batch to its indirect batch set (8 bytes).
///
/// Mirrors Bevy's `IndirectParametersCpuMetadata` for GPU compute preprocessing pipelines.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct IndirectParametersCpuMetadata {
    /// Starting instance index in the output instance buffer.
    pub base_output_index: u32,
    /// Zero-based batch-set index, or `u32::MAX` when not part of a multi-draw batch set.
    pub batch_set_index: u32,
}

/// Mesh geometry bounds used to construct [`DrawIndirectArgs`] or [`DrawIndexedIndirectArgs`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum IndirectMeshMetadata {
    /// Indexed mesh bounds (`first_index .. first_index + index_count` plus `base_vertex`).
    Indexed {
        /// Contiguous index range within the bound index buffer.
        index_range: Range<u32>,
        /// Signed vertex offset added to each index.
        base_vertex: i32,
    },
    /// Non-indexed mesh bounds (`first_vertex .. first_vertex + vertex_count`).
    NonIndexed {
        /// Contiguous vertex range within bound vertex buffers.
        vertex_range: Range<u32>,
    },
}

impl IndirectMeshMetadata {
    /// Whether this metadata describes an indexed draw.
    #[inline]
    pub const fn is_indexed(&self) -> bool {
        matches!(self, Self::Indexed { .. })
    }

    /// Builds [`DrawIndexedIndirectArgs`] for `instance_range` if this metadata is [`Self::Indexed`].
    #[inline]
    pub fn to_indexed_args(&self, instance_range: &Range<u32>) -> Option<DrawIndexedIndirectArgs> {
        match self {
            Self::Indexed {
                index_range,
                base_vertex,
            } => Some(DrawIndexedIndirectArgs {
                index_count: index_range.end.saturating_sub(index_range.start),
                instance_count: instance_range.end.saturating_sub(instance_range.start),
                first_index: index_range.start,
                base_vertex: *base_vertex,
                first_instance: instance_range.start,
            }),
            Self::NonIndexed { .. } => None,
        }
    }

    /// Builds [`DrawIndirectArgs`] for `instance_range` if this metadata is [`Self::NonIndexed`].
    #[inline]
    pub fn to_non_indexed_args(&self, instance_range: &Range<u32>) -> Option<DrawIndirectArgs> {
        match self {
            Self::NonIndexed { vertex_range } => Some(DrawIndirectArgs {
                vertex_count: vertex_range.end.saturating_sub(vertex_range.start),
                instance_count: instance_range.end.saturating_sub(instance_range.start),
                first_vertex: vertex_range.start,
                first_instance: instance_range.start,
            }),
            Self::Indexed { .. } => None,
        }
    }
}
