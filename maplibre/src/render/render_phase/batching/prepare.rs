use std::{cmp::Ordering, collections::HashMap, ops::Range};

use super::{
    unbatched::{prepare_non_mesh_bins, prepare_unbatchable_bins},
    BatchedInstanceBuffer, BinnedRenderPhaseBatch, BinnedRenderPhaseBatchSet,
    BinnedRenderPhaseBatchSets, FallbackInstanceBufferMode, GetBatchData,
};
use crate::{
    context::MapContext,
    render::{
        render_phase::{
            binned::{
                BinnedItemId, BinnedPhaseItem, BinnedRenderPhase, InputDataIndex,
                PhaseItemBatchSetKey, RenderBin,
            },
            draw::PhaseItemExtraIndex,
            indirect::{IndirectMeshMetadata, IndirectParametersBuffers},
        },
        tracked_pass::DrawCapabilities,
    },
    tcs::{
        system::{SystemError, SystemResult},
        world::World,
    },
};

struct PreparedBinInfo<BK> {
    bin_key: BK,
    representative_item: BinnedItemId,
    instance_range: Range<u32>,
    metadata: Option<IndirectMeshMetadata>,
}

/// Pure-CPU batching and indirect parameter preparation for `BinnedRenderPhase<BPI>`.
///
/// Exposed directly so CPU unit tests and custom schedules can run the exact `PhaseSort`
/// batching pipeline against any [`DrawCapabilities`] and uniform alignment without a GPU device.
pub fn batch_and_prepare_binned_render_phase_cpu<BPI, GBD>(
    world: &mut World,
    capabilities: DrawCapabilities,
    min_uniform_alignment: u32,
) where
    BPI: BinnedPhaseItem,
    GBD: GetBatchData<BPI>,
{
    let Some(mut phase) = world
        .resources
        .get_mut::<BinnedRenderPhase<BPI>>()
        .map(std::mem::take)
    else {
        return;
    };
    let mut instance_buffer = world
        .resources
        .get_mut::<BatchedInstanceBuffer<GBD::BufferData>>()
        .map(std::mem::take)
        .unwrap_or_default();
    let mut indirect_buffers = world
        .resources
        .get_mut::<IndirectParametersBuffers>()
        .map(std::mem::take)
        .unwrap_or_default();

    phase.prepared_unbatchable.clear();
    phase.prepared_non_mesh.clear();
    let mut out_sets = phase.batch_sets.take_backing_vec();

    let use_dynamic_uniforms = !capabilities.vertex_storage_buffers
        && instance_buffer.fallback_mode() == FallbackInstanceBufferMode::DynamicUniform;
    let use_multidraw_mode = !use_dynamic_uniforms
        && capabilities.multi_draw_indirect
        && capabilities.indirect_execution
        && capabilities.indirect_first_instance;

    let mut md_groups: Vec<(BPI::BatchSetKey, Vec<BPI::BinKey>)> = phase
        .multidrawable_meshes
        .iter()
        .filter_map(|(bsk, bins)| {
            let mut active_bins: Vec<BPI::BinKey> = bins
                .iter()
                .filter(|(_, bin)| !bin.entries.is_empty())
                .map(|(bk, _)| bk.clone())
                .collect();
            if active_bins.is_empty() {
                None
            } else {
                active_bins.sort_unstable();
                Some((bsk.clone(), active_bins))
            }
        })
        .collect();
    md_groups.sort_unstable_by(|a, b| a.0.cmp(&b.0));

    let mut batchable_keys: Vec<(BPI::BatchSetKey, BPI::BinKey)> = phase
        .batchable_meshes
        .iter()
        .filter(|(_, bin)| !bin.entries.is_empty())
        .map(|((bsk, bk), _)| (bsk.clone(), bk.clone()))
        .collect();
    batchable_keys.sort_unstable();

    let mut md_idx = 0usize;
    let mut ba_idx = 0usize;

    while md_idx < md_groups.len() || ba_idx < batchable_keys.len() {
        let take_md = match (md_groups.get(md_idx), batchable_keys.get(ba_idx)) {
            (Some((md_bsk, md_bins)), Some((ba_bsk, ba_bk))) => match md_bsk.cmp(ba_bsk) {
                Ordering::Less => true,
                Ordering::Greater => false,
                Ordering::Equal => md_bins[0] <= *ba_bk,
            },
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => break,
        };

        if take_md {
            let (bsk, sorted_bin_keys) = &md_groups[md_idx];
            md_idx += 1;
            let bins_map = phase
                .multidrawable_meshes
                .get(bsk)
                .expect("multidraw group exists");

            if use_dynamic_uniforms {
                let mut batches = Vec::new();
                for bk in sorted_bin_keys {
                    let bin = bins_map.get(bk).expect("bin exists");
                    append_dynamic_uniform_bin::<BPI, GBD>(
                        world,
                        bk,
                        bin,
                        &mut instance_buffer,
                        min_uniform_alignment,
                        &mut batches,
                    );
                }
                push_batches_coalesced(&mut out_sets, bsk.clone(), batches);
            } else {
                prepare_multidraw_group::<BPI, GBD>(
                    world,
                    bsk,
                    sorted_bin_keys,
                    bins_map,
                    capabilities,
                    use_multidraw_mode,
                    &mut instance_buffer,
                    &mut indirect_buffers,
                    &mut out_sets,
                );
            }
        } else {
            let (bsk, bk) = &batchable_keys[ba_idx];
            ba_idx += 1;
            let bin = phase
                .batchable_meshes
                .get(&(bsk.clone(), bk.clone()))
                .expect("batchable bin exists");

            if use_dynamic_uniforms {
                let mut batches = Vec::new();
                append_dynamic_uniform_bin::<BPI, GBD>(
                    world,
                    bk,
                    bin,
                    &mut instance_buffer,
                    min_uniform_alignment,
                    &mut batches,
                );
                push_batches_coalesced(&mut out_sets, bsk.clone(), batches);
            } else if let Some(info) =
                pack_contiguous_bin::<BPI, GBD>(world, bk, bin, &mut instance_buffer)
            {
                let extra_index = build_single_extra_index(
                    bsk.indexed(),
                    capabilities,
                    info.metadata.as_ref(),
                    &info.instance_range,
                    &mut indirect_buffers,
                );
                let batch = BinnedRenderPhaseBatch {
                    representative_item: info.representative_item,
                    instance_range: info.instance_range,
                    extra_index,
                };
                push_batches_coalesced(&mut out_sets, bsk.clone(), vec![(info.bin_key, batch)]);
            }
        }
    }

    phase.batch_sets = if use_dynamic_uniforms {
        BinnedRenderPhaseBatchSets::DynamicUniforms(out_sets)
    } else if use_multidraw_mode {
        BinnedRenderPhaseBatchSets::MultidrawIndirect(out_sets)
    } else {
        BinnedRenderPhaseBatchSets::Single(out_sets)
    };

    prepare_unbatchable_bins::<BPI, GBD>(
        world,
        &phase.unbatchable_meshes,
        use_dynamic_uniforms,
        min_uniform_alignment,
        &mut instance_buffer,
        &mut phase.prepared_unbatchable,
    );
    prepare_non_mesh_bins::<BPI, GBD>(
        world,
        &phase.non_mesh_items,
        use_dynamic_uniforms,
        min_uniform_alignment,
        &mut instance_buffer,
        &mut phase.prepared_non_mesh,
    );

    world.resources.insert(phase);
    world.resources.insert(instance_buffer);
    world.resources.insert(indirect_buffers);
}

#[allow(clippy::too_many_arguments)]
fn prepare_multidraw_group<BPI: BinnedPhaseItem, GBD: GetBatchData<BPI>>(
    world: &World,
    bsk: &BPI::BatchSetKey,
    sorted_bin_keys: &[BPI::BinKey],
    bins_map: &HashMap<BPI::BinKey, RenderBin>,
    capabilities: DrawCapabilities,
    use_multidraw_mode: bool,
    instance_buffer: &mut BatchedInstanceBuffer<GBD::BufferData>,
    indirect_buffers: &mut IndirectParametersBuffers,
    out_sets: &mut Vec<BinnedRenderPhaseBatchSet<BPI>>,
) {
    let mut prepared_bins = Vec::with_capacity(sorted_bin_keys.len());
    for bk in sorted_bin_keys {
        let bin = bins_map.get(bk).expect("bin exists");
        if let Some(info) = pack_contiguous_bin::<BPI, GBD>(world, bk, bin, instance_buffer) {
            prepared_bins.push(info);
        }
    }
    if prepared_bins.is_empty() {
        return;
    }
    let indexed = bsk.indexed();
    let all_have_matching_meta = prepared_bins.iter().all(|b| {
        b.metadata
            .as_ref()
            .is_some_and(|meta| meta.is_indexed() == indexed)
    });

    if use_multidraw_mode
        && capabilities.supports_multidraw_indirect(indexed)
        && all_have_matching_meta
    {
        let range = if indexed {
            indirect_buffers.extend_indexed(prepared_bins.iter().map(|b| {
                b.metadata
                    .as_ref()
                    .expect("checked")
                    .to_indexed_args(&b.instance_range)
                    .expect("checked indexed")
            }))
        } else {
            indirect_buffers.extend_non_indexed(prepared_bins.iter().map(|b| {
                b.metadata
                    .as_ref()
                    .expect("checked")
                    .to_non_indexed_args(&b.instance_range)
                    .expect("checked non-indexed")
            }))
        };
        let batch_count = range.end.saturating_sub(range.start);
        let batch_set_index = indirect_buffers.push_batch_set(batch_count, range.start);
        let first = &prepared_bins[0];
        let last = prepared_bins.last().expect("non-empty");
        out_sets.push(BinnedRenderPhaseBatchSet::MultidrawIndirect {
            batch_set_key: bsk.clone(),
            bin_key: first.bin_key.clone(),
            representative_item: first.representative_item,
            instance_range: first.instance_range.start..last.instance_range.end,
            indirect_parameters_index: range.start,
            batch_count,
            batch_set_index,
        });
    } else {
        let batches = prepared_bins
            .into_iter()
            .map(|info| {
                let extra_index = build_single_extra_index(
                    indexed,
                    capabilities,
                    info.metadata.as_ref(),
                    &info.instance_range,
                    indirect_buffers,
                );
                (
                    info.bin_key,
                    BinnedRenderPhaseBatch {
                        representative_item: info.representative_item,
                        instance_range: info.instance_range,
                        extra_index,
                    },
                )
            })
            .collect();
        push_batches_coalesced(out_sets, bsk.clone(), batches);
    }
}

fn push_batches_coalesced<BPI: BinnedPhaseItem>(
    out_sets: &mut Vec<BinnedRenderPhaseBatchSet<BPI>>,
    batch_set_key: BPI::BatchSetKey,
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
        if *last_bsk == batch_set_key {
            batches.append(&mut new_batches);
            return;
        }
    }
    out_sets.push(BinnedRenderPhaseBatchSet::Batches {
        batch_set_key,
        batches: new_batches,
    });
}

fn pack_contiguous_bin<BPI: BinnedPhaseItem, GBD: GetBatchData<BPI>>(
    world: &World,
    bin_key: &BPI::BinKey,
    bin: &RenderBin,
    instance_buffer: &mut BatchedInstanceBuffer<GBD::BufferData>,
) -> Option<PreparedBinInfo<BPI::BinKey>> {
    let mut start_idx: Option<u32> = None;
    let mut end_idx: u32 = 0;
    let mut representative_item = BinnedItemId(0);
    let mut metadata: Option<IndirectMeshMetadata> = None;

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

    let start = start_idx?;
    Some(PreparedBinInfo {
        bin_key: bin_key.clone(),
        representative_item,
        instance_range: start..end_idx,
        metadata,
    })
}

fn append_dynamic_uniform_bin<BPI: BinnedPhaseItem, GBD: GetBatchData<BPI>>(
    world: &World,
    bin_key: &BPI::BinKey,
    bin: &RenderBin,
    instance_buffer: &mut BatchedInstanceBuffer<GBD::BufferData>,
    min_uniform_alignment: u32,
    out_batches: &mut Vec<(BPI::BinKey, BinnedRenderPhaseBatch)>,
) {
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

fn build_single_extra_index(
    indexed: bool,
    capabilities: DrawCapabilities,
    metadata: Option<&IndirectMeshMetadata>,
    instance_range: &Range<u32>,
    indirect_buffers: &mut IndirectParametersBuffers,
) -> PhaseItemExtraIndex {
    if !capabilities.supports_indirect_batching(indexed) {
        return PhaseItemExtraIndex::None;
    }
    let Some(meta) = metadata else {
        return PhaseItemExtraIndex::None;
    };
    if meta.is_indexed() != indexed {
        tracing::warn!(
            indexed,
            "IndirectMeshMetadata variant did not match PhaseItemBatchSetKey::indexed(); falling back to direct instancing"
        );
        return PhaseItemExtraIndex::None;
    }
    let slot = if indexed {
        let args = meta
            .to_indexed_args(instance_range)
            .expect("checked indexed");
        indirect_buffers.push_indexed(args)
    } else {
        let args = meta
            .to_non_indexed_args(instance_range)
            .expect("checked non-indexed");
        indirect_buffers.push_non_indexed(args)
    };
    PhaseItemExtraIndex::IndirectParametersIndex {
        range: slot..(slot + 1),
        batch_set_index: None,
    }
}

/// Scheduled `PhaseSort` system that batches instances on the CPU and uploads instance and
/// indirect parameter buffers to the GPU via [`UploadQueue`](crate::render::upload_queue::UploadQueue).
pub fn batch_and_prepare_binned_render_phase<BPI, GBD>(
    MapContext {
        world, renderer, ..
    }: &mut MapContext,
) -> SystemResult
where
    BPI: BinnedPhaseItem,
    GBD: GetBatchData<BPI>,
{
    if !world.resources.exists::<BinnedRenderPhase<BPI>>() {
        return Err(SystemError::Dependencies);
    }
    let capabilities = world
        .resources
        .get::<DrawCapabilities>()
        .copied()
        .unwrap_or_else(|| {
            DrawCapabilities::from_adapter_and_device(&renderer.adapter, &renderer.device)
        });
    let min_uniform_alignment = renderer.device.limits().min_uniform_buffer_offset_alignment;

    batch_and_prepare_binned_render_phase_cpu::<BPI, GBD>(
        world,
        capabilities,
        min_uniform_alignment,
    );

    if let Some(instance_buffer) = world
        .resources
        .get_mut::<BatchedInstanceBuffer<GBD::BufferData>>()
    {
        instance_buffer.upload(&renderer.device, &renderer.queue, capabilities);
    }
    if let Some(indirect_buffers) = world.resources.get_mut::<IndirectParametersBuffers>() {
        indirect_buffers.upload(&renderer.device, &renderer.queue, capabilities);
    }
    Ok(())
}
