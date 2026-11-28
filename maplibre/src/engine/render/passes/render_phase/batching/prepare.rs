use std::cmp::Ordering;

mod bins;

use self::bins::{
    append_dynamic_uniform_bins, build_single_extra_index, can_group_multidraw,
    pack_contiguous_bins, prepare_multidraw_group, push_batches_coalesced,
};
use super::{
    unbatched::{prepare_non_mesh_bins, prepare_unbatchable_bins},
    BatchedInstanceBuffer, BinnedRenderPhaseBatch, BinnedRenderPhaseBatchSet,
    BinnedRenderPhaseBatchSets, FallbackInstanceBufferMode, GetBatchData,
};
use crate::{
    render::{
        render_phase::{
            binned::{BinnedPhaseItem, BinnedRenderPhase, PhaseItemBatchSetKey, RenderBin},
            indirect_buffers::IndirectParametersBuffers,
        },
        tracked_pass::DrawCapabilities,
    },
    tcs::world::World,
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
    phase.prepare_cpu::<GBD>(world, capabilities, min_uniform_alignment);
    world.resources.insert(phase);
}

impl<BPI: BinnedPhaseItem> BinnedRenderPhase<BPI> {
    pub(crate) fn prepare_cpu<GBD: GetBatchData<BPI>>(
        &mut self,
        world: &mut World,
        capabilities: DrawCapabilities,
        min_uniform_alignment: u32,
    ) {
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

        self.prepared_unbatchable.clear();
        self.prepared_non_mesh.clear();
        let mut out_sets = self.batch_sets.take_backing_vec();

        let fallback_mode = self
            .fallback_mode
            .unwrap_or_else(|| instance_buffer.fallback_mode());
        let mode = BatchPrepMode::new(capabilities, fallback_mode, min_uniform_alignment);
        let groups = self.collect_sorted_groups::<GBD>(world, mode);
        prepare_batch_sets::<BPI, GBD>(
            world,
            groups,
            mode,
            (&mut instance_buffer, &mut indirect_buffers, &mut out_sets),
        );
        self.batch_sets = if mode.use_dynamic_uniforms {
            BinnedRenderPhaseBatchSets::DynamicUniforms(out_sets)
        } else if mode.use_multidraw_mode {
            BinnedRenderPhaseBatchSets::MultidrawIndirect(out_sets)
        } else {
            BinnedRenderPhaseBatchSets::Single(out_sets)
        };
        prepare_unbatchable_bins::<BPI, GBD>(
            world,
            &self.unbatchable_meshes,
            mode.use_dynamic_uniforms,
            mode.use_vertex_instance,
            min_uniform_alignment,
            &mut instance_buffer,
            &mut self.prepared_unbatchable,
        );
        prepare_non_mesh_bins::<BPI, GBD>(
            world,
            &self.non_mesh_items,
            mode.use_dynamic_uniforms,
            mode.use_vertex_instance,
            min_uniform_alignment,
            &mut instance_buffer,
            &mut self.prepared_non_mesh,
        );

        world.resources.insert(instance_buffer);
        world.resources.insert(indirect_buffers);
    }

    fn collect_sorted_groups<'a, GBD: GetBatchData<BPI>>(
        &'a self,
        world: &World,
        mode: BatchPrepMode,
    ) -> (
        Vec<BorrowedMdGroup<'a, BPI>>,
        Vec<BorrowedSingleBin<'a, BPI>>,
    ) {
        let mut md_groups: Vec<BorrowedMdGroup<'a, BPI>> = Vec::new();
        let mut single_bins: Vec<BorrowedSingleBin<'a, BPI>> = Vec::new();
        for (bsk, bins_map) in &self.multidrawable_meshes {
            let mut active: Vec<(&BPI::BinKey, &RenderBin)> = bins_map
                .iter()
                .filter(|(_, bin)| !bin.entries.is_empty())
                .collect();
            if active.is_empty() {
                continue;
            }
            active.sort_unstable_by(|a, b| a.0.cmp(b.0));
            if mode.use_multidraw_mode
                && mode.capabilities.supports_multidraw_indirect(bsk.indexed())
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
        for ((bsk, bk), bin) in &self.batchable_meshes {
            if !bin.entries.is_empty() {
                single_bins.push((bsk, bk, bin, 1));
            }
        }
        single_bins.sort_unstable_by(|a, b| (a.0, a.1, a.3).cmp(&(b.0, b.1, b.3)));
        (md_groups, single_bins)
    }
}

#[derive(Clone, Copy)]
struct BatchPrepMode {
    capabilities: DrawCapabilities,
    min_uniform_alignment: u32,
    use_dynamic_uniforms: bool,
    use_vertex_instance: bool,
    use_multidraw_mode: bool,
}

impl BatchPrepMode {
    fn new(
        capabilities: DrawCapabilities,
        fallback_mode: FallbackInstanceBufferMode,
        min_uniform_alignment: u32,
    ) -> Self {
        let use_dynamic_uniforms = !capabilities.vertex_storage_buffers
            && fallback_mode == FallbackInstanceBufferMode::DynamicUniform;
        let use_vertex_instance = !capabilities.vertex_storage_buffers
            && fallback_mode == FallbackInstanceBufferMode::VertexInstance;
        let use_multidraw_mode = !use_dynamic_uniforms
            && !use_vertex_instance
            && capabilities.multi_draw_indirect
            && capabilities.indirect_execution
            && capabilities.indirect_first_instance;
        Self {
            capabilities,
            min_uniform_alignment,
            use_dynamic_uniforms,
            use_vertex_instance,
            use_multidraw_mode,
        }
    }
}

fn prepare_batch_sets<'a, BPI: BinnedPhaseItem, GBD: GetBatchData<BPI>>(
    world: &World,
    (md_groups, single_bins): (
        Vec<BorrowedMdGroup<'a, BPI>>,
        Vec<BorrowedSingleBin<'a, BPI>>,
    ),
    mode: BatchPrepMode,
    (instance_buffer, indirect_buffers, out_sets): (
        &mut BatchedInstanceBuffer<GBD::BufferData>,
        &mut IndirectParametersBuffers,
        &mut Vec<BinnedRenderPhaseBatchSet<BPI>>,
    ),
) {
    let mut md_idx = 0usize;
    let mut ba_idx = 0usize;
    let mut same_key_bins: Vec<&RenderBin> = Vec::new();
    loop {
        let md_next = md_groups.get(md_idx);
        let ba_next = single_bins.get(ba_idx);
        let take_md = match (md_next, ba_next) {
            (Some((md_bsk, md_bins)), Some((ba_bsk, ba_bk, _, _))) => {
                match (*md_bsk).cmp(*ba_bsk) {
                    Ordering::Less => true,
                    Ordering::Greater => false,
                    Ordering::Equal => md_bins.first().is_some_and(|(bk, _)| *bk <= *ba_bk),
                }
            }
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => break,
        };
        if take_md {
            let Some((bsk, sorted_bins)) = md_next else {
                break;
            };
            md_idx += 1;
            prepare_multidraw_group::<BPI, GBD>(
                world,
                bsk,
                sorted_bins,
                instance_buffer,
                indirect_buffers,
                out_sets,
            );
            continue;
        }
        let Some(&(bsk, bk, first_bin, _)) = ba_next else {
            break;
        };
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
        prepare_single_key_bins::<BPI, GBD>(
            world,
            bsk,
            bk,
            &same_key_bins,
            mode,
            (instance_buffer, indirect_buffers, out_sets),
        );
    }
}

fn prepare_single_key_bins<BPI: BinnedPhaseItem, GBD: GetBatchData<BPI>>(
    world: &World,
    bsk: &BPI::BatchSetKey,
    bk: &BPI::BinKey,
    same_key_bins: &[&RenderBin],
    mode: BatchPrepMode,
    (instance_buffer, indirect_buffers, out_sets): (
        &mut BatchedInstanceBuffer<GBD::BufferData>,
        &mut IndirectParametersBuffers,
        &mut Vec<BinnedRenderPhaseBatchSet<BPI>>,
    ),
) {
    if mode.use_dynamic_uniforms {
        let mut batches = Vec::new();
        append_dynamic_uniform_bins::<BPI, GBD>(
            world,
            bk,
            same_key_bins,
            instance_buffer,
            mode.min_uniform_alignment,
            &mut batches,
        );
        push_batches_coalesced(out_sets, bsk, batches);
    } else if let Some(info) =
        pack_contiguous_bins::<BPI, GBD>(world, bk, same_key_bins, instance_buffer)
    {
        let extra_index = build_single_extra_index(
            bsk.indexed(),
            mode.use_vertex_instance,
            mode.capabilities,
            info.metadata.as_ref(),
            &info.instance_range,
            indirect_buffers,
        );
        let batch = BinnedRenderPhaseBatch {
            representative_item: info.representative_item,
            instance_range: info.instance_range,
            extra_index,
        };
        push_batches_coalesced(out_sets, bsk, vec![(info.bin_key, batch)]);
    }
}
