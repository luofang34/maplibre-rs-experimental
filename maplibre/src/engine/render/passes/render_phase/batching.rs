//! CPU/GPU instance batching traits, prepared batch sets, and `PhaseSort` systems.

mod instance_buffer;
mod prepare;
mod unbatched;

pub use instance_buffer::BatchedInstanceBuffer;
pub use prepare::batch_and_prepare_binned_render_phase_cpu;

pub use super::binned::{
    BinnedRenderPhaseBatch, BinnedRenderPhaseBatchSet, BinnedRenderPhaseBatchSets,
    FallbackInstanceBufferMode,
};
use super::{
    binned::{BinnedItemId, BinnedPhaseItem, InputDataIndex},
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

#[cfg(test)]
mod tests;
