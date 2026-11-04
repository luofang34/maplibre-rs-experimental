use std::cmp::Ordering;

mod bins;

use self::bins::{
    append_dynamic_uniform_bins, build_single_extra_index, can_group_multidraw,
    pack_contiguous_bins, prepare_multidraw_group, push_batches_coalesced,
};
use super::{
    unbatched::{prepare_non_mesh_bins, prepare_unbatchable_bins},
    BatchedInstanceBuffer, BinnedRenderPhaseBatch, BinnedRenderPhaseBatchSets,
    FallbackInstanceBufferMode, GetBatchData,
};
use crate::{
    context::MapContext,
    render::{
        render_phase::{
            binned::{BinnedPhaseItem, BinnedRenderPhase, PhaseItemBatchSetKey, RenderBin},
            indirect::IndirectParametersBuffers,
        },
        tracked_pass::DrawCapabilities,
    },
    tcs::{
        system::{SystemError, SystemResult},
        world::World,
    },
};

type BorrowedMdGroup<'a, BPI> = (
    &'a <BPI as BinnedPhaseItem>::BatchSetKey,
    Vec<(&'a <BPI as BinnedPhaseItem>::BinKey, &'a RenderBin)>,
);
type BorrowedSingleBin<'a, BPI> = (
    &'a <BPI as BinnedPhaseItem>::BatchSetKey,
    &'a <BPI as BinnedPhaseItem>::BinKey,
    &'a RenderBin,
    u8,
);

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

    let fallback_mode = phase
        .fallback_mode
        .unwrap_or_else(|| instance_buffer.fallback_mode());
    let use_dynamic_uniforms = !capabilities.vertex_storage_buffers
        && fallback_mode == FallbackInstanceBufferMode::DynamicUniform;
    let use_vertex_instance = !capabilities.vertex_storage_buffers
        && fallback_mode == FallbackInstanceBufferMode::VertexInstance;
    let use_multidraw_mode = !use_dynamic_uniforms
        && !use_vertex_instance
        && capabilities.multi_draw_indirect
        && capabilities.indirect_execution
        && capabilities.indirect_first_instance;

    {
        let mut md_groups: Vec<BorrowedMdGroup<'_, BPI>> = Vec::new();
        let mut single_bins: Vec<BorrowedSingleBin<'_, BPI>> = Vec::new();

        for (bsk, bins_map) in &phase.multidrawable_meshes {
            let mut active: Vec<(&BPI::BinKey, &RenderBin)> = bins_map
                .iter()
                .filter(|(_, bin)| !bin.entries.is_empty())
                .collect();
            if active.is_empty() {
                continue;
            }
            active.sort_unstable_by(|a, b| a.0.cmp(b.0));
            if use_multidraw_mode
                && capabilities.supports_multidraw_indirect(bsk.indexed())
                && can_group_multidraw::<BPI, GBD>(world, bsk.indexed(), &active)
            {
                md_groups.push((bsk, active));
            } else {
                for (bk, bin) in active {
                    single_bins.push((bsk, bk, bin, 0));
                }
            }
        }
        md_groups.sort_unstable_by(|a, b| a.0.cmp(b.0));

        for ((bsk, bk), bin) in &phase.batchable_meshes {
            if !bin.entries.is_empty() {
                single_bins.push((bsk, bk, bin, 1));
            }
        }
        single_bins.sort_unstable_by(|a, b| (a.0, a.1, a.3).cmp(&(b.0, b.1, b.3)));

        let mut md_idx = 0usize;
        let mut ba_idx = 0usize;
        let mut same_key_bins: Vec<&RenderBin> = Vec::new();

        while md_idx < md_groups.len() || ba_idx < single_bins.len() {
            let take_md = match (md_groups.get(md_idx), single_bins.get(ba_idx)) {
                (Some((md_bsk, md_bins)), Some((ba_bsk, ba_bk, _, _))) => {
                    match (*md_bsk).cmp(*ba_bsk) {
                        Ordering::Less => true,
                        Ordering::Greater => false,
                        Ordering::Equal => md_bins[0].0 <= *ba_bk,
                    }
                }
                (Some(_), None) => true,
                (None, Some(_)) => false,
                (None, None) => break,
            };

            if take_md {
                let (bsk, sorted_bins) = &md_groups[md_idx];
                md_idx += 1;
                prepare_multidraw_group::<BPI, GBD>(
                    world,
                    bsk,
                    sorted_bins,
                    &mut instance_buffer,
                    &mut indirect_buffers,
                    &mut out_sets,
                );
            } else {
                let (bsk, bk, first_bin, _) = single_bins[ba_idx];
                ba_idx += 1;
                same_key_bins.clear();
                same_key_bins.push(first_bin);
                while let Some(&(next_bsk, next_bk, next_bin, _)) = single_bins.get(ba_idx) {
                    if next_bsk == bsk && next_bk == bk {
                        same_key_bins.push(next_bin);
                        ba_idx += 1;
                    } else {
                        break;
                    }
                }

                if use_dynamic_uniforms {
                    let mut batches = Vec::new();
                    append_dynamic_uniform_bins::<BPI, GBD>(
                        world,
                        bk,
                        &same_key_bins,
                        &mut instance_buffer,
                        min_uniform_alignment,
                        &mut batches,
                    );
                    push_batches_coalesced(&mut out_sets, bsk, batches);
                } else if let Some(info) = pack_contiguous_bins::<BPI, GBD>(
                    world,
                    bk,
                    &same_key_bins,
                    &mut instance_buffer,
                ) {
                    let extra_index = build_single_extra_index(
                        bsk.indexed(),
                        use_vertex_instance,
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
                    push_batches_coalesced(&mut out_sets, bsk, vec![(info.bin_key, batch)]);
                }
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
        use_vertex_instance,
        min_uniform_alignment,
        &mut instance_buffer,
        &mut phase.prepared_unbatchable,
    );
    prepare_non_mesh_bins::<BPI, GBD>(
        world,
        &phase.non_mesh_items,
        use_dynamic_uniforms,
        use_vertex_instance,
        min_uniform_alignment,
        &mut instance_buffer,
        &mut phase.prepared_non_mesh,
    );

    world.resources.insert(phase);
    world.resources.insert(instance_buffer);
    world.resources.insert(indirect_buffers);
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
