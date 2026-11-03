use std::{fmt::Debug, hash::Hash, ops::Range};

use crate::render::render_phase::{
    draw::{DrawFunctionId, PhaseItemExtraIndex},
    ProjectionBinding,
};

/// Opaque identifier of a queued entity, feature, or mesh block in a [`BinnedRenderPhase`](super::BinnedRenderPhase).
///
/// Serves the role of Bevy's `(Entity, MainEntity)` representative handle without tying
/// [`BinnedRenderPhase`](super::BinnedRenderPhase) to a specific ECS entity representation.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BinnedItemId(pub u64);

/// Zero-based index into a caller's per-frame input instance table prepared during `Queue`.
///
/// Corresponds to Bevy's `InputUniformIndex`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InputDataIndex(pub u32);

/// Outer key grouping compatible bins into a batch set capable of sharing pipeline state and,
/// when supported, coalescing into a single multi-draw indirect call.
///
/// Mirrors Bevy's `PhaseItemBatchSetKey`.
pub trait PhaseItemBatchSetKey: Clone + Eq + Ord + Hash + Debug + 'static {
    /// Whether draws in this batch set use an index buffer (`draw_indexed*`) rather than
    /// non-indexed vertex draws (`draw*`).
    fn indexed(&self) -> bool;

    /// Identifier of the [`Draw`](crate::render::render_phase::Draw) function in
    /// [`DrawFunctions`](crate::render::render_phase::DrawFunctions) that records this batch set.
    fn draw_function(&self) -> DrawFunctionId;
}

/// Phase item constructed on the stack per merged batch during [`BinnedRenderPhase::render`](super::BinnedRenderPhase::render).
///
/// Mirrors Bevy's `BinnedPhaseItem`: individual queued instances are stored compactly inside
/// reusable bins rather than materialized as full `BinnedPhaseItem` structs during `Queue`.
pub trait BinnedPhaseItem: Sized + 'static {
    /// Outer key grouping bins that share pipeline/slab state and can multi-draw together.
    type BatchSetKey: PhaseItemBatchSetKey;

    /// Inner key grouping identical drawable meshes/materials within a [`Self::BatchSetKey`] into a
    /// contiguous instanced batch.
    type BinKey: Clone + Eq + Ord + Hash + Debug + 'static;

    /// Constructs a batch representative on the stack immediately before invoking its draw function.
    fn new(
        batch_set_key: Self::BatchSetKey,
        bin_key: Self::BinKey,
        representative_item: BinnedItemId,
        batch_range: Range<u32>,
        extra_index: PhaseItemExtraIndex,
    ) -> Self;

    /// Contiguous instance range in [`BatchedInstanceBuffer`](crate::render::render_phase::BatchedInstanceBuffer) drawn by this batch.
    fn batch_range(&self) -> &Range<u32>;

    /// Supplementary dynamic uniform offset or indirect parameter slot index for this batch.
    fn extra_index(&self) -> PhaseItemExtraIndex;

    /// [`DrawFunctionId`] looked up in [`DrawFunctions<Self>`](crate::render::render_phase::DrawFunctions) to record this batch.
    fn draw_function(&self) -> DrawFunctionId;

    /// Optional projection uniform binding at group zero; defaults to [`ProjectionBinding::View`].
    fn projection_binding(&self) -> ProjectionBinding {
        ProjectionBinding::View
    }
}

/// Classification determining how [`BinnedRenderPhase::add`](super::BinnedRenderPhase::add) bins and batches an incoming item.
///
/// Mirrors Bevy's `BinnedRenderPhaseType`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum BinnedRenderPhaseType {
    /// Coalesces instances within `(BatchSetKey, BinKey)` and, when
    /// [`DrawCapabilities::supports_multidraw_indirect`](crate::render::tracked_pass::DrawCapabilities::supports_multidraw_indirect)
    /// holds, merges multiple `BinKey`s under the same `BatchSetKey` into one `multi_draw_[indexed_]indirect` call.
    MultidrawableMesh,
    /// Coalesces instances within the same `(BatchSetKey, BinKey)` bin without merging across `BinKey`s.
    BatchableMesh,
    /// Cannot merge instance buffers (e.g. per-item [`PhaseItemExtraIndex::DynamicOffset`]), but groups by
    /// `(BatchSetKey, BinKey)` so shared pipeline and material bindings deduplicate in
    /// [`TrackedRenderPass`](crate::render::tracked_pass::TrackedRenderPass).
    UnbatchableMesh,
    /// Custom or procedural non-mesh geometry grouped by `(BatchSetKey, BinKey)` to minimize state changes.
    NonMesh,
}

impl BinnedRenderPhaseType {
    /// Selects the mesh phase classification from `batchable` and `multidrawable` flags, matching
    /// Bevy's `BinnedRenderPhaseType::mesh`.
    #[inline]
    pub const fn mesh(batchable: bool, multidrawable: bool) -> Self {
        match (batchable, multidrawable) {
            (true, true) => Self::MultidrawableMesh,
            (true, false) => Self::BatchableMesh,
            (false, _) => Self::UnbatchableMesh,
        }
    }
}
