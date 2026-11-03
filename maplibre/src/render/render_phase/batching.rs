//! CPU/GPU instance batching traits, prepared batch sets, and `PhaseSort` systems.

use std::ops::Range;

mod instance_buffer;
mod prepare;
mod unbatched;

pub use instance_buffer::BatchedInstanceBuffer;
pub use prepare::{
    batch_and_prepare_binned_render_phase, batch_and_prepare_binned_render_phase_cpu,
};

use super::{
    binned::{BinnedItemId, BinnedPhaseItem, InputDataIndex},
    draw::PhaseItemExtraIndex,
    indirect::IndirectMeshMetadata,
};
use crate::tcs::world::World;

/// Extracts per-instance GPU data (and optional indirect mesh bounds) from [`World`] during `PhaseSort`.
///
/// Mirrors Bevy's `GetBatchData`.
pub trait GetBatchData<BPI: BinnedPhaseItem>: 'static {
    /// Plain-old-data instance payload written into [`BatchedInstanceBuffer`].
    type BufferData: bytemuck::Pod + bytemuck::Zeroable;

    /// Fetches the GPU instance payload for `item` at `input_index`, optionally returning
    /// [`IndirectMeshMetadata`] when the mesh supports indirect draw submission.
    fn get_batch_data(
        world: &World,
        item: BinnedItemId,
        input_index: InputDataIndex,
    ) -> Option<(Self::BufferData, Option<IndirectMeshMetadata>)>;
}

/// Extension of [`GetBatchData`] for GPU-driven compute preprocessing and culling pipelines.
///
/// Mirrors Bevy's `GetFullBatchData`, separating raw per-instance input uniforms from mesh
/// indirect bounds so compute passes can populate indirect draw counts on the GPU.
pub trait GetFullBatchData<BPI: BinnedPhaseItem>: GetBatchData<BPI> {
    /// Raw input structure uploaded before GPU compute expansion/culling.
    type BufferInputData: bytemuck::Pod + bytemuck::Zeroable;

    /// Fetches the raw input payload for `item` at `input_index`.
    fn get_binned_input_data(
        world: &World,
        item: BinnedItemId,
        input_index: InputDataIndex,
    ) -> Option<Self::BufferInputData>;

    /// Fetches indirect draw geometry bounds for a bin's representative item.
    fn get_binned_indirect_metadata(
        world: &World,
        item: BinnedItemId,
    ) -> Option<IndirectMeshMetadata>;
}

/// Fallback GPU buffer strategy used by [`BatchedInstanceBuffer`] when vertex storage buffers
/// are unavailable (e.g. WebGL2 / strict downlevel backends).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum FallbackInstanceBufferMode {
    /// Packs each instance aligned to `min_uniform_buffer_offset_alignment` into a
    /// `UNIFORM | COPY_DST` buffer and emits [`PhaseItemExtraIndex::DynamicOffset`] per item.
    #[default]
    DynamicUniform,
    /// Packs instances tightly into a `VERTEX | COPY_DST` buffer for `VertexStepMode::Instance`
    /// vertex attributes without storage buffer bindings.
    VertexInstance,
}

/// Prepared batch metadata for a single `(BatchSetKey, BinKey)` bin (or single dynamic-offset item).
///
/// Mirrors Bevy's `BinnedRenderPhaseBatch`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BinnedRenderPhaseBatch {
    /// Representative item handle from this bin used to resolve mesh/material resources.
    pub representative_item: BinnedItemId,
    /// Contiguous instance range in [`BatchedInstanceBuffer`] drawn by this batch.
    pub instance_range: Range<u32>,
    /// Optional dynamic uniform offset or single-batch indirect parameter slot.
    pub extra_index: PhaseItemExtraIndex,
}

/// One prepared batch set sharing a [`BinnedPhaseItem::BatchSetKey`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BinnedRenderPhaseBatchSet<BPI: BinnedPhaseItem> {
    /// Multi-draw indirect batch set coalescing one or more `MultidrawableMesh` bins under
    /// `batch_set_key` into a contiguous range of [`IndirectParametersBuffers`](super::IndirectParametersBuffers).
    MultidrawIndirect {
        /// Shared outer batch-set key.
        batch_set_key: BPI::BatchSetKey,
        /// Representative bin key (first bin in deterministic `(BatchSetKey, BinKey)` order).
        bin_key: BPI::BinKey,
        /// Representative item from the first bin.
        representative_item: BinnedItemId,
        /// Combined contiguous instance range across all bins in this batch set.
        instance_range: Range<u32>,
        /// Starting slot index in [`IndirectParametersBuffers`](super::IndirectParametersBuffers).
        indirect_parameters_index: u32,
        /// Number of contiguous indirect draw commands in this batch set.
        batch_count: u32,
        /// 1-based slot index into [`IndirectParametersBuffers::batch_sets_buffer`](super::IndirectParametersBuffers::batch_sets_buffer).
        batch_set_index: std::num::NonZeroU32,
    },
    /// Individual per-bin (or per-dynamic-offset) batches sharing `batch_set_key`.
    Batches {
        /// Shared outer batch-set key.
        batch_set_key: BPI::BatchSetKey,
        /// Deterministically ordered `(BinKey, BinnedRenderPhaseBatch)` pairs.
        batches: Vec<(BPI::BinKey, BinnedRenderPhaseBatch)>,
    },
}

/// Prepared batch-set collection selected according to [`DrawCapabilities`](crate::render::tracked_pass::DrawCapabilities).
///
/// Mirrors Bevy's `BinnedRenderPhaseBatchSets`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BinnedRenderPhaseBatchSets<BPI: BinnedPhaseItem> {
    /// Downlevel / WebGL2 dynamic uniform offset batches.
    DynamicUniforms(Vec<BinnedRenderPhaseBatchSet<BPI>>),
    /// Per-bin instanced or single-indirect batches.
    Single(Vec<BinnedRenderPhaseBatchSet<BPI>>),
    /// Multi-draw indirect batch sets for `MultidrawableMesh` bins (plus per-bin sets for any
    /// `BatchableMesh` bins in the same phase).
    MultidrawIndirect(Vec<BinnedRenderPhaseBatchSet<BPI>>),
}

impl<BPI: BinnedPhaseItem> Default for BinnedRenderPhaseBatchSets<BPI> {
    fn default() -> Self {
        Self::Single(Vec::new())
    }
}

impl<BPI: BinnedPhaseItem> BinnedRenderPhaseBatchSets<BPI> {
    /// Borrows the prepared batch sets regardless of active submission strategy.
    #[inline]
    pub fn as_slice(&self) -> &[BinnedRenderPhaseBatchSet<BPI>] {
        match self {
            Self::DynamicUniforms(sets) | Self::Single(sets) | Self::MultidrawIndirect(sets) => {
                sets.as_slice()
            }
        }
    }

    /// Whether no batch sets are currently prepared.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.as_slice().is_empty()
    }

    /// Clears prepared batch sets while preserving outer vector capacity.
    pub fn clear(&mut self) {
        match self {
            Self::DynamicUniforms(sets) | Self::Single(sets) | Self::MultidrawIndirect(sets) => {
                sets.clear();
            }
        }
    }

    pub(super) fn take_backing_vec(&mut self) -> Vec<BinnedRenderPhaseBatchSet<BPI>> {
        match self {
            Self::DynamicUniforms(sets) | Self::Single(sets) | Self::MultidrawIndirect(sets) => {
                let mut v = std::mem::take(sets);
                v.clear();
                v
            }
        }
    }
}

#[cfg(test)]
mod tests;
