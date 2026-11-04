use std::collections::HashMap;

use super::{BatchedInstanceBuffer, GetBatchData};
use crate::{
    render::render_phase::{
        binned::{
            BinnedPhaseItem, InputDataIndex, PreparedBinGroupList, PreparedSingleEntry, RenderBin,
        },
        draw::PhaseItemExtraIndex,
    },
    tcs::world::World,
};

fn sorted_active_bins<BPI: BinnedPhaseItem>(
    bins_map: &HashMap<(BPI::BatchSetKey, BPI::BinKey), RenderBin>,
) -> Vec<(&BPI::BatchSetKey, &BPI::BinKey, &RenderBin)> {
    let mut active: Vec<(&BPI::BatchSetKey, &BPI::BinKey, &RenderBin)> = bins_map
        .iter()
        .filter(|(_, bin)| !bin.entries.is_empty())
        .map(|((bsk, bk), bin)| (bsk, bk, bin))
        .collect();
    active.sort_unstable_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
    active
}

pub(super) fn prepare_unbatchable_bins<BPI: BinnedPhaseItem, GBD: GetBatchData<BPI>>(
    world: &World,
    bins_map: &HashMap<(BPI::BatchSetKey, BPI::BinKey), RenderBin>,
    use_dynamic_uniforms: bool,
    use_vertex_instance: bool,
    min_uniform_alignment: u32,
    instance_buffer: &mut BatchedInstanceBuffer<GBD::BufferData>,
    out: &mut PreparedBinGroupList<BPI>,
) {
    let active_bins = sorted_active_bins::<BPI>(bins_map);

    for (bsk, bk, bin) in active_bins {
        let group_start = out.entries.len();
        for entry in &bin.entries {
            let count = entry.instance_count.get();
            for offset in 0..count {
                let input = InputDataIndex(entry.input_index.0.wrapping_add(offset));
                let has_explicit_extra = entry.extra_index != PhaseItemExtraIndex::None;
                match GBD::get_batch_data(world, entry.item, input) {
                    Some((bd, _)) => {
                        if has_explicit_extra {
                            if entry.extra_index.dynamic_offset().is_some() {
                                if !use_dynamic_uniforms && !use_vertex_instance {
                                    let _ = instance_buffer.push(bd);
                                }
                                out.entries.push(PreparedSingleEntry {
                                    item: entry.item,
                                    instance_range: 0..1,
                                    extra_index: entry.extra_index.clone(),
                                });
                            } else {
                                let idx = instance_buffer.push(bd);
                                out.entries.push(PreparedSingleEntry {
                                    item: entry.item,
                                    instance_range: idx..(idx + 1),
                                    extra_index: entry.extra_index.clone(),
                                });
                            }
                        } else if use_dynamic_uniforms {
                            let (_, dyn_off) =
                                instance_buffer.push_dynamic_uniform(bd, min_uniform_alignment);
                            out.entries.push(PreparedSingleEntry {
                                item: entry.item,
                                instance_range: 0..1,
                                extra_index: PhaseItemExtraIndex::DynamicOffset(dyn_off),
                            });
                        } else {
                            let idx = instance_buffer.push(bd);
                            let extra_index = if use_vertex_instance {
                                PhaseItemExtraIndex::VertexInstance
                            } else {
                                PhaseItemExtraIndex::None
                            };
                            out.entries.push(PreparedSingleEntry {
                                item: entry.item,
                                instance_range: idx..(idx + 1),
                                extra_index,
                            });
                        }
                    }
                    None if has_explicit_extra => {
                        out.entries.push(PreparedSingleEntry {
                            item: entry.item,
                            instance_range: 0..1,
                            extra_index: entry.extra_index.clone(),
                        });
                    }
                    None => {}
                }
            }
        }
        let group_end = out.entries.len();
        if group_end > group_start {
            out.groups
                .push((bsk.clone(), bk.clone(), group_start..group_end));
        }
    }
}

pub(super) fn prepare_non_mesh_bins<BPI: BinnedPhaseItem, GBD: GetBatchData<BPI>>(
    world: &World,
    bins_map: &HashMap<(BPI::BatchSetKey, BPI::BinKey), RenderBin>,
    use_dynamic_uniforms: bool,
    use_vertex_instance: bool,
    min_uniform_alignment: u32,
    instance_buffer: &mut BatchedInstanceBuffer<GBD::BufferData>,
    out: &mut PreparedBinGroupList<BPI>,
) {
    let active_bins = sorted_active_bins::<BPI>(bins_map);

    for (bsk, bk, bin) in active_bins {
        let group_start = out.entries.len();
        for entry in &bin.entries {
            let count = entry.instance_count.get();
            if use_dynamic_uniforms {
                let mut matched_any = false;
                for offset in 0..count {
                    let input = InputDataIndex(entry.input_index.0.wrapping_add(offset));
                    if let Some((bd, _)) = GBD::get_batch_data(world, entry.item, input) {
                        matched_any = true;
                        let (_, dyn_off) =
                            instance_buffer.push_dynamic_uniform(bd, min_uniform_alignment);
                        out.entries.push(PreparedSingleEntry {
                            item: entry.item,
                            instance_range: 0..1,
                            extra_index: PhaseItemExtraIndex::DynamicOffset(dyn_off),
                        });
                    }
                }
                if !matched_any {
                    out.entries.push(PreparedSingleEntry {
                        item: entry.item,
                        instance_range: 0..count,
                        extra_index: entry.extra_index.clone(),
                    });
                }
            } else {
                let mut start_idx: Option<u32> = None;
                let mut end_idx = 0u32;
                for offset in 0..count {
                    let input = InputDataIndex(entry.input_index.0.wrapping_add(offset));
                    if let Some((bd, _)) = GBD::get_batch_data(world, entry.item, input) {
                        let idx = instance_buffer.push(bd);
                        if start_idx.is_none() {
                            start_idx = Some(idx);
                        }
                        end_idx = idx + 1;
                    }
                }
                let (instance_range, extra_index) = match start_idx {
                    Some(s) => {
                        let extra = if entry.extra_index == PhaseItemExtraIndex::None
                            && use_vertex_instance
                        {
                            PhaseItemExtraIndex::VertexInstance
                        } else {
                            entry.extra_index.clone()
                        };
                        (s..end_idx, extra)
                    }
                    None => (0..count, entry.extra_index.clone()),
                };
                out.entries.push(PreparedSingleEntry {
                    item: entry.item,
                    instance_range,
                    extra_index,
                });
            }
        }
        let group_end = out.entries.len();
        if group_end > group_start {
            out.groups
                .push((bsk.clone(), bk.clone(), group_start..group_end));
        }
    }
}
