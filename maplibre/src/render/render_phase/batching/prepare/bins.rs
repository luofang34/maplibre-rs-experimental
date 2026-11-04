use std::ops::Range;

use crate::{
    render::{
        render_phase::{
            batching::{
                BatchedInstanceBuffer, BinnedRenderPhaseBatch, BinnedRenderPhaseBatchSet,
                GetBatchData,
            },
            binned::{
                BinnedItemId, BinnedPhaseItem, InputDataIndex, PhaseItemBatchSetKey, RenderBin,
            },
            draw::PhaseItemExtraIndex,
            indirect::{IndirectMeshMetadata, IndirectParametersBuffers},
        },
        tracked_pass::DrawCapabilities,
    },
    tcs::world::World,
};

pub(super) struct PreparedBinInfo<BK> {
    pub(super) bin_key: BK,
    pub(super) representative_item: BinnedItemId,
    pub(super) instance_range: Range<u32>,
    pub(super) metadata: Option<IndirectMeshMetadata>,
}

fn first_bin_metadata<BPI: BinnedPhaseItem, GBD: GetBatchData<BPI>>(
    world: &World,
    bin: &RenderBin,
) -> Option<Option<IndirectMeshMetadata>> {
    for entry in &bin.entries {
        let count = entry.instance_count.get();
        for offset in 0..count {
            let input = InputDataIndex(entry.input_index.0.wrapping_add(offset));
            if let Some((_, meta)) = GBD::get_batch_data(world, entry.item, input) {
                return Some(meta);
            }
        }
    }
    None
}

pub(super) fn can_group_multidraw<BPI: BinnedPhaseItem, GBD: GetBatchData<BPI>>(
    world: &World,
    indexed: bool,
    active_bins: &[(&BPI::BinKey, &RenderBin)],
) -> bool {
    let mut any_valid = false;
    for (_, bin) in active_bins {
        if let Some(meta) = first_bin_metadata::<BPI, GBD>(world, bin) {
            any_valid = true;
            if meta.is_none_or(|m| m.is_indexed() != indexed) {
                return false;
            }
        }
    }
    any_valid
}

pub(super) fn prepare_multidraw_group<BPI: BinnedPhaseItem, GBD: GetBatchData<BPI>>(
    world: &World,
    bsk: &BPI::BatchSetKey,
    sorted_bins: &[(&BPI::BinKey, &RenderBin)],
    instance_buffer: &mut BatchedInstanceBuffer<GBD::BufferData>,
    indirect_buffers: &mut IndirectParametersBuffers,
    out_sets: &mut Vec<BinnedRenderPhaseBatchSet<BPI>>,
) {
    let mut prepared_bins = Vec::with_capacity(sorted_bins.len());
    for (bk, bin) in sorted_bins {
        if let Some(info) = pack_contiguous_bins::<BPI, GBD>(world, bk, &[*bin], instance_buffer) {
            prepared_bins.push(info);
        }
    }
    let (Some(first), Some(last)) = (prepared_bins.first(), prepared_bins.last()) else {
        return;
    };
    let indexed = bsk.indexed();
    let range = if indexed {
        indirect_buffers.extend_indexed(prepared_bins.iter().filter_map(|b| {
            b.metadata
                .as_ref()
                .and_then(|meta| meta.to_indexed_args(&b.instance_range))
        }))
    } else {
        indirect_buffers.extend_non_indexed(prepared_bins.iter().filter_map(|b| {
            b.metadata
                .as_ref()
                .and_then(|meta| meta.to_non_indexed_args(&b.instance_range))
        }))
    };
    let batch_count = range.end.saturating_sub(range.start);
    if batch_count == 0 {
        return;
    }
    let batch_set_index = indirect_buffers.push_batch_set(batch_count, range.start);
    out_sets.push(BinnedRenderPhaseBatchSet::MultidrawIndirect {
        batch_set_key: bsk.clone(),
        bin_key: first.bin_key.clone(),
        representative_item: first.representative_item,
        instance_range: first.instance_range.start..last.instance_range.end,
        indirect_parameters_index: range.start,
        batch_count,
        batch_set_index,
    });
}

pub(super) fn push_batches_coalesced<BPI: BinnedPhaseItem>(
    out_sets: &mut Vec<BinnedRenderPhaseBatchSet<BPI>>,
    batch_set_key: &BPI::BatchSetKey,
    mut new_batches: Vec<(BPI::BinKey, BinnedRenderPhaseBatch)>,
) {
    if new_batches.is_empty() {
        return;
    }
    if let Some(BinnedRenderPhaseBatchSet::Batches {
        batch_set_key: last_bsk,
        batches,
    }) = out_sets.last_mut()
    {
        if last_bsk == batch_set_key {
            batches.append(&mut new_batches);
            return;
        }
    }
    out_sets.push(BinnedRenderPhaseBatchSet::Batches {
        batch_set_key: batch_set_key.clone(),
        batches: new_batches,
    });
}

pub(super) fn pack_contiguous_bins<BPI: BinnedPhaseItem, GBD: GetBatchData<BPI>>(
    world: &World,
    bin_key: &BPI::BinKey,
    bins: &[&RenderBin],
    instance_buffer: &mut BatchedInstanceBuffer<GBD::BufferData>,
) -> Option<PreparedBinInfo<BPI::BinKey>> {
    let mut start_idx: Option<u32> = None;
    let mut end_idx: u32 = 0;
    let mut representative_item = BinnedItemId(0);
    let mut metadata: Option<IndirectMeshMetadata> = None;

    for bin in bins {
        for entry in &bin.entries {
            let count = entry.instance_count.get();
            for offset in 0..count {
                let input = InputDataIndex(entry.input_index.0.wrapping_add(offset));
                if let Some((bd, item_meta)) = GBD::get_batch_data(world, entry.item, input) {
                    let idx = instance_buffer.push(bd);
                    if start_idx.is_none() {
                        start_idx = Some(idx);
                        representative_item = entry.item;
                        metadata = item_meta;
                    }
                    end_idx = idx + 1;
                }
            }
        }
    }

    let start = start_idx?;
    Some(PreparedBinInfo {
        bin_key: bin_key.clone(),
        representative_item,
        instance_range: start..end_idx,
        metadata,
    })
}

pub(super) fn append_dynamic_uniform_bins<BPI: BinnedPhaseItem, GBD: GetBatchData<BPI>>(
    world: &World,
    bin_key: &BPI::BinKey,
    bins: &[&RenderBin],
    instance_buffer: &mut BatchedInstanceBuffer<GBD::BufferData>,
    min_uniform_alignment: u32,
    out_batches: &mut Vec<(BPI::BinKey, BinnedRenderPhaseBatch)>,
) {
    for bin in bins {
        for entry in &bin.entries {
            let count = entry.instance_count.get();
            for offset in 0..count {
                let input = InputDataIndex(entry.input_index.0.wrapping_add(offset));
                if let Some((bd, _)) = GBD::get_batch_data(world, entry.item, input) {
                    let (_, dyn_offset) =
                        instance_buffer.push_dynamic_uniform(bd, min_uniform_alignment);
                    out_batches.push((
                        bin_key.clone(),
                        BinnedRenderPhaseBatch {
                            representative_item: entry.item,
                            instance_range: 0..1,
                            extra_index: PhaseItemExtraIndex::DynamicOffset(dyn_offset),
                        },
                    ));
                }
            }
        }
    }
}

pub(super) fn build_single_extra_index(
    indexed: bool,
    use_vertex_instance: bool,
    capabilities: DrawCapabilities,
    metadata: Option<&IndirectMeshMetadata>,
    instance_range: &Range<u32>,
    indirect_buffers: &mut IndirectParametersBuffers,
) -> PhaseItemExtraIndex {
    if use_vertex_instance {
        return PhaseItemExtraIndex::VertexInstance;
    }
    if !capabilities.supports_indirect_batching(indexed) {
        return PhaseItemExtraIndex::None;
    }
    let Some(meta) = metadata else {
        return PhaseItemExtraIndex::None;
    };
    let maybe_slot = if indexed {
        meta.to_indexed_args(instance_range)
            .map(|args| indirect_buffers.push_indexed(args))
    } else {
        meta.to_non_indexed_args(instance_range)
            .map(|args| indirect_buffers.push_non_indexed(args))
    };
    let Some(slot) = maybe_slot else {
        tracing::warn!(
            indexed,
            "IndirectMeshMetadata variant did not match PhaseItemBatchSetKey::indexed(); falling back to direct instancing"
        );
        return PhaseItemExtraIndex::None;
    };
    PhaseItemExtraIndex::IndirectParametersIndex {
        range: slot..(slot + 1),
        batch_set_index: None,
    }
}
