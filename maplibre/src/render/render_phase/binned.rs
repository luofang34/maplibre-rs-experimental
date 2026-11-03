//! O(1) binned render phase, dual-level bin keys, sweep lifecycle, and pass recording.

use std::{collections::HashMap, num::NonZeroU32, ops::Range};

mod dispatch;
mod keys;

pub use dispatch::{
    draw_binned_indexed, draw_binned_mesh, draw_binned_non_indexed, DirectDrawGeometry,
};
pub use keys::{
    BinnedItemId, BinnedPhaseItem, BinnedRenderPhaseType, InputDataIndex, PhaseItemBatchSetKey,
};

use super::{
    batching::{BinnedRenderPhaseBatchSet, BinnedRenderPhaseBatchSets},
    draw::{DrawFunctions, PhaseItemExtraIndex},
};
use crate::{render::tracked_pass::TrackedRenderPass, tcs::world::World};

/// Default number of inactive sweep generations an empty bin survives before its bucket is freed.
pub const DEFAULT_MAX_INACTIVE_GENERATIONS: u64 = 2;

#[derive(Clone, Debug)]
pub(super) struct BinEntry {
    pub(super) item: BinnedItemId,
    pub(super) input_index: InputDataIndex,
    pub(super) instance_count: NonZeroU32,
    pub(super) extra_index: PhaseItemExtraIndex,
}

#[derive(Debug)]
pub(super) struct RenderBin {
    pub(super) entries: Vec<BinEntry>,
    pub(super) last_active_generation: u64,
}

impl RenderBin {
    fn new(generation: u64) -> Self {
        Self {
            entries: Vec::new(),
            last_active_generation: generation,
        }
    }

    #[inline]
    fn push(&mut self, entry: BinEntry, generation: u64) {
        self.last_active_generation = generation;
        self.entries.push(entry);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PreparedSingleItem<BPI: BinnedPhaseItem> {
    pub(super) batch_set_key: BPI::BatchSetKey,
    pub(super) bin_key: BPI::BinKey,
    pub(super) item: BinnedItemId,
    pub(super) instance_range: Range<u32>,
    pub(super) extra_index: PhaseItemExtraIndex,
}

/// O(1) binned render phase supporting instanced coalescing, multi-draw indirect submission,
/// and cross-frame bucket capacity retention.
pub struct BinnedRenderPhase<BPI: BinnedPhaseItem> {
    pub(super) multidrawable_meshes: HashMap<BPI::BatchSetKey, HashMap<BPI::BinKey, RenderBin>>,
    pub(super) batchable_meshes: HashMap<(BPI::BatchSetKey, BPI::BinKey), RenderBin>,
    pub(super) unbatchable_meshes: HashMap<(BPI::BatchSetKey, BPI::BinKey), RenderBin>,
    pub(super) non_mesh_items: HashMap<(BPI::BatchSetKey, BPI::BinKey), RenderBin>,
    pub(super) batch_sets: BinnedRenderPhaseBatchSets<BPI>,
    pub(super) prepared_unbatchable: Vec<PreparedSingleItem<BPI>>,
    pub(super) prepared_non_mesh: Vec<PreparedSingleItem<BPI>>,
    generation: u64,
    max_inactive_generations: u64,
    pub(super) last_swept_eye_frame: Option<u64>,
}

impl<BPI: BinnedPhaseItem> Default for BinnedRenderPhase<BPI> {
    fn default() -> Self {
        Self::new()
    }
}

impl<BPI: BinnedPhaseItem> BinnedRenderPhase<BPI> {
    /// Creates an empty [`BinnedRenderPhase`] with [`DEFAULT_MAX_INACTIVE_GENERATIONS`].
    pub fn new() -> Self {
        Self {
            multidrawable_meshes: HashMap::new(),
            batchable_meshes: HashMap::new(),
            unbatchable_meshes: HashMap::new(),
            non_mesh_items: HashMap::new(),
            batch_sets: BinnedRenderPhaseBatchSets::default(),
            prepared_unbatchable: Vec::new(),
            prepared_non_mesh: Vec::new(),
            generation: 0,
            max_inactive_generations: DEFAULT_MAX_INACTIVE_GENERATIONS,
            last_swept_eye_frame: None,
        }
    }

    /// Configures how many consecutive inactive sweep generations a bin survives before removal.
    pub fn with_max_inactive_generations(mut self, max_inactive_generations: u64) -> Self {
        self.max_inactive_generations = max_inactive_generations;
        self
    }

    /// Updates how many consecutive inactive sweep generations a bin survives before removal.
    pub fn set_max_inactive_generations(&mut self, max_inactive_generations: u64) {
        self.max_inactive_generations = max_inactive_generations;
    }

    /// Maximum inactive sweep generations before a cold bin is evicted by [`Self::sweep`].
    #[inline]
    pub const fn max_inactive_generations(&self) -> u64 {
        self.max_inactive_generations
    }

    /// Current sweep generation counter.
    #[inline]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Inserts a single-instance item into the matching bin in amortized O(1) time.
    #[inline]
    pub fn add(
        &mut self,
        batch_set_key: BPI::BatchSetKey,
        bin_key: BPI::BinKey,
        item: BinnedItemId,
        input_index: InputDataIndex,
        phase_type: BinnedRenderPhaseType,
    ) {
        self.add_with_extra_index(
            batch_set_key,
            bin_key,
            item,
            input_index,
            NonZeroU32::MIN,
            PhaseItemExtraIndex::None,
            phase_type,
        );
    }

    /// Inserts a pre-aggregated contiguous block of `instance_count` instances (`input_index .. input_index + count`).
    #[inline]
    pub fn add_instanced(
        &mut self,
        batch_set_key: BPI::BatchSetKey,
        bin_key: BPI::BinKey,
        item: BinnedItemId,
        input_index: InputDataIndex,
        instance_count: NonZeroU32,
        phase_type: BinnedRenderPhaseType,
    ) {
        self.add_with_extra_index(
            batch_set_key,
            bin_key,
            item,
            input_index,
            instance_count,
            PhaseItemExtraIndex::None,
            phase_type,
        );
    }

    /// Inserts an item (or instance span) carrying an explicit [`PhaseItemExtraIndex`] such as
    /// [`PhaseItemExtraIndex::DynamicOffset`] for [`BinnedRenderPhaseType::UnbatchableMesh`].
    #[allow(clippy::too_many_arguments)]
    pub fn add_with_extra_index(
        &mut self,
        batch_set_key: BPI::BatchSetKey,
        bin_key: BPI::BinKey,
        item: BinnedItemId,
        input_index: InputDataIndex,
        instance_count: NonZeroU32,
        extra_index: PhaseItemExtraIndex,
        phase_type: BinnedRenderPhaseType,
    ) {
        let generation = self.generation;
        let entry = BinEntry {
            item,
            input_index,
            instance_count,
            extra_index,
        };
        match phase_type {
            BinnedRenderPhaseType::MultidrawableMesh => {
                self.multidrawable_meshes
                    .entry(batch_set_key)
                    .or_default()
                    .entry(bin_key)
                    .or_insert_with(|| RenderBin::new(generation))
                    .push(entry, generation);
            }
            BinnedRenderPhaseType::BatchableMesh => {
                self.batchable_meshes
                    .entry((batch_set_key, bin_key))
                    .or_insert_with(|| RenderBin::new(generation))
                    .push(entry, generation);
            }
            BinnedRenderPhaseType::UnbatchableMesh => {
                self.unbatchable_meshes
                    .entry((batch_set_key, bin_key))
                    .or_insert_with(|| RenderBin::new(generation))
                    .push(entry, generation);
            }
            BinnedRenderPhaseType::NonMesh => {
                self.non_mesh_items
                    .entry((batch_set_key, bin_key))
                    .or_insert_with(|| RenderBin::new(generation))
                    .push(entry, generation);
            }
        }
    }

    /// Prepared batch sets produced during `PhaseSort`.
    #[inline]
    pub fn batch_sets(&self) -> &BinnedRenderPhaseBatchSets<BPI> {
        &self.batch_sets
    }

    /// Whether neither queued bin entries nor prepared draw batches remain in this phase.
    pub fn is_empty(&self) -> bool {
        self.batch_sets.is_empty()
            && self.prepared_unbatchable.is_empty()
            && self.prepared_non_mesh.is_empty()
            && self
                .multidrawable_meshes
                .values()
                .flat_map(|bins| bins.values())
                .all(|bin| bin.entries.is_empty())
            && self
                .batchable_meshes
                .values()
                .all(|bin| bin.entries.is_empty())
            && self
                .unbatchable_meshes
                .values()
                .all(|bin| bin.entries.is_empty())
            && self
                .non_mesh_items
                .values()
                .all(|bin| bin.entries.is_empty())
    }

    /// Total number of resident bin buckets currently retained across all four category maps.
    pub fn allocated_bin_count(&self) -> usize {
        let multidraw: usize = self.multidrawable_meshes.values().map(HashMap::len).sum();
        multidraw
            + self.batchable_meshes.len()
            + self.unbatchable_meshes.len()
            + self.non_mesh_items.len()
    }

    /// Returns the retained `Vec::capacity()` of a specific bin if it is still resident.
    pub fn bin_capacity(
        &self,
        batch_set_key: &BPI::BatchSetKey,
        bin_key: &BPI::BinKey,
        phase_type: BinnedRenderPhaseType,
    ) -> Option<usize> {
        let lookup = |map: &HashMap<(BPI::BatchSetKey, BPI::BinKey), RenderBin>| -> Option<usize> {
            map.iter().find_map(|((bsk, bk), bin)| {
                (bsk == batch_set_key && bk == bin_key).then_some(bin.entries.capacity())
            })
        };
        match phase_type {
            BinnedRenderPhaseType::MultidrawableMesh => self
                .multidrawable_meshes
                .get(batch_set_key)
                .and_then(|bins| bins.get(bin_key))
                .map(|bin| bin.entries.capacity()),
            BinnedRenderPhaseType::BatchableMesh => lookup(&self.batchable_meshes),
            BinnedRenderPhaseType::UnbatchableMesh => lookup(&self.unbatchable_meshes),
            BinnedRenderPhaseType::NonMesh => lookup(&self.non_mesh_items),
        }
    }

    /// Clears all queued entries and prepared batches for the current pass/viewport while
    /// retaining every bin's hash node and `Vec` capacity.
    pub fn clear_entries(&mut self) {
        for bins in self.multidrawable_meshes.values_mut() {
            for bin in bins.values_mut() {
                bin.entries.clear();
            }
        }
        for bin in self.batchable_meshes.values_mut() {
            bin.entries.clear();
        }
        for bin in self.unbatchable_meshes.values_mut() {
            bin.entries.clear();
        }
        for bin in self.non_mesh_items.values_mut() {
            bin.entries.clear();
        }
        self.batch_sets.clear();
        self.prepared_unbatchable.clear();
        self.prepared_non_mesh.clear();
    }

    /// Clears current-frame draw state, advances the frame generation counter, and evicts cold
    /// bins that have been empty for more than [`Self::max_inactive_generations`] sweeps.
    pub fn sweep(&mut self) {
        self.clear_entries();
        let current = self.generation;
        let max_age = self.max_inactive_generations;
        let keep = |bin: &RenderBin| current.saturating_sub(bin.last_active_generation) <= max_age;

        self.multidrawable_meshes.retain(|_, bins| {
            bins.retain(|_, bin| keep(bin));
            !bins.is_empty()
        });
        self.batchable_meshes.retain(|_, bin| keep(bin));
        self.unbatchable_meshes.retain(|_, bin| keep(bin));
        self.non_mesh_items.retain(|_, bin| keep(bin));
        self.generation = self.generation.wrapping_add(1);
    }

    /// Materializes each prepared batch or unbatched item on the stack in deterministic draw order.
    pub fn for_each_render_item(&self, mut visit: impl FnMut(BPI)) {
        for set in self.batch_sets.as_slice() {
            match set {
                BinnedRenderPhaseBatchSet::MultidrawIndirect {
                    batch_set_key,
                    bin_key,
                    representative_item,
                    instance_range,
                    indirect_parameters_index,
                    batch_count,
                    batch_set_index,
                } => {
                    if *batch_count == 0 {
                        continue;
                    }
                    let range =
                        *indirect_parameters_index..(*indirect_parameters_index + *batch_count);
                    visit(BPI::new(
                        batch_set_key.clone(),
                        bin_key.clone(),
                        *representative_item,
                        instance_range.clone(),
                        PhaseItemExtraIndex::IndirectParametersIndex {
                            range,
                            batch_set_index: Some(*batch_set_index),
                        },
                    ));
                }
                BinnedRenderPhaseBatchSet::Batches {
                    batch_set_key,
                    batches,
                } => {
                    for (bin_key, batch) in batches {
                        visit(BPI::new(
                            batch_set_key.clone(),
                            bin_key.clone(),
                            batch.representative_item,
                            batch.instance_range.clone(),
                            batch.extra_index.clone(),
                        ));
                    }
                }
            }
        }
        for entry in &self.prepared_unbatchable {
            visit(BPI::new(
                entry.batch_set_key.clone(),
                entry.bin_key.clone(),
                entry.item,
                entry.instance_range.clone(),
                entry.extra_index.clone(),
            ));
        }
        for entry in &self.prepared_non_mesh {
            visit(BPI::new(
                entry.batch_set_key.clone(),
                entry.bin_key.clone(),
                entry.item,
                entry.instance_range.clone(),
                entry.extra_index.clone(),
            ));
        }
    }

    /// Records all prepared batches and items in this phase into `pass` using [`DrawFunctions<BPI>`].
    pub fn render<'w>(&self, pass: &mut TrackedRenderPass<'w>, world: &'w World) {
        let Some(draw_functions) = world.resources.get::<DrawFunctions<BPI>>() else {
            return;
        };
        self.for_each_render_item(|item| {
            if let Some(draw_fn) = draw_functions.get(item.draw_function()) {
                draw_fn.draw(pass, world, &item);
            }
        });
    }
}

#[cfg(test)]
mod tests;
