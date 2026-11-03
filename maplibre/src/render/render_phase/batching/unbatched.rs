use std::collections::HashMap;

use super::{BatchedInstanceBuffer, GetBatchData};
use crate::{
    render::render_phase::{
        binned::{BinnedPhaseItem, InputDataIndex, PreparedSingleItem, RenderBin},
        draw::PhaseItemExtraIndex,
    },
    tcs::world::World,
};

pub(super) fn prepare_unbatchable_bins<BPI: BinnedPhaseItem, GBD: GetBatchData<BPI>>(
    world: &World,
    bins_map: &HashMap<(BPI::BatchSetKey, BPI::BinKey), RenderBin>,
    use_dynamic_uniforms: bool,
    min_uniform_alignment: u32,
    instance_buffer: &mut BatchedInstanceBuffer<GBD::BufferData>,
    out: &mut Vec<PreparedSingleItem<BPI>>,
) {
    let mut keys: Vec<(BPI::BatchSetKey, BPI::BinKey)> = bins_map
        .iter()
        .filter(|(_, bin)| !bin.entries.is_empty())
        .map(|((bsk, bk), _)| (bsk.clone(), bk.clone()))
        .collect();
    keys.sort_unstable();

    for (bsk, bk) in keys {
        let bin = bins_map.get(&(bsk.clone(), bk.clone())).expect("exists");
        for entry in &bin.entries {
            let count = entry.instance_count.get();
            for offset in 0..count {
                let input = InputDataIndex(entry.input_index.0.wrapping_add(offset));
                let has_explicit_extra = entry.extra_index != PhaseItemExtraIndex::None;
                match GBD::get_batch_data(world, entry.item, input) {
                    Some((bd, _)) => {
                        if has_explicit_extra {
                            let idx = instance_buffer.push(bd);
                            let instance_range = if entry.extra_index.dynamic_offset().is_some() {
                                0..1
                            } else {
                                idx..(idx + 1)
                            };
                            out.push(PreparedSingleItem {
                                batch_set_key: bsk.clone(),
                                bin_key: bk.clone(),
                                item: entry.item,
                                instance_range,
                                extra_index: entry.extra_index.clone(),
                            });
                        } else if use_dynamic_uniforms {
                            let (_, dyn_off) =
                                instance_buffer.push_dynamic_uniform(bd, min_uniform_alignment);
                            out.push(PreparedSingleItem {
                                batch_set_key: bsk.clone(),
                                bin_key: bk.clone(),
                                item: entry.item,
                                instance_range: 0..1,
                                extra_index: PhaseItemExtraIndex::DynamicOffset(dyn_off),
                            });
                        } else {
                            let idx = instance_buffer.push(bd);
                            out.push(PreparedSingleItem {
                                batch_set_key: bsk.clone(),
                                bin_key: bk.clone(),
                                item: entry.item,
                                instance_range: idx..(idx + 1),
                                extra_index: PhaseItemExtraIndex::None,
                            });
                        }
                    }
                    None if has_explicit_extra => {
                        out.push(PreparedSingleItem {
                            batch_set_key: bsk.clone(),
                            bin_key: bk.clone(),
                            item: entry.item,
                            instance_range: 0..1,
                            extra_index: entry.extra_index.clone(),
                        });
                    }
                    None => {}
                }
            }
        }
    }
}

pub(super) fn prepare_non_mesh_bins<BPI: BinnedPhaseItem, GBD: GetBatchData<BPI>>(
    world: &World,
    bins_map: &HashMap<(BPI::BatchSetKey, BPI::BinKey), RenderBin>,
    use_dynamic_uniforms: bool,
    min_uniform_alignment: u32,
    instance_buffer: &mut BatchedInstanceBuffer<GBD::BufferData>,
    out: &mut Vec<PreparedSingleItem<BPI>>,
) {
    let mut keys: Vec<(BPI::BatchSetKey, BPI::BinKey)> = bins_map
        .iter()
        .filter(|(_, bin)| !bin.entries.is_empty())
        .map(|((bsk, bk), _)| (bsk.clone(), bk.clone()))
        .collect();
    keys.sort_unstable();

    for (bsk, bk) in keys {
        let bin = bins_map.get(&(bsk.clone(), bk.clone())).expect("exists");
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
                        out.push(PreparedSingleItem {
                            batch_set_key: bsk.clone(),
                            bin_key: bk.clone(),
                            item: entry.item,
                            instance_range: 0..1,
                            extra_index: PhaseItemExtraIndex::DynamicOffset(dyn_off),
                        });
                    }
                }
                if !matched_any {
                    out.push(PreparedSingleItem {
                        batch_set_key: bsk.clone(),
                        bin_key: bk.clone(),
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
                let instance_range = start_idx.map_or(0..count, |s| s..end_idx);
                out.push(PreparedSingleItem {
                    batch_set_key: bsk.clone(),
                    bin_key: bk.clone(),
                    item: entry.item,
                    instance_range,
                    extra_index: entry.extra_index.clone(),
                });
            }
        }
    }
}
