mod fallbacks;

use std::{num::NonZeroU32, ops::Range};

use bytemuck_derive::{Pod, Zeroable};

use crate::{
    render::{
        render_phase::{
            batch_and_prepare_binned_render_phase_cpu, BatchedInstanceBuffer, BinnedItemId,
            BinnedPhaseItem, BinnedRenderPhase, BinnedRenderPhaseBatchSet,
            BinnedRenderPhaseBatchSets, BinnedRenderPhaseType, DrawFunctionId,
            DrawIndexedIndirectArgs, DrawIndirectArgs, GetBatchData, GetFullBatchData,
            IndirectBatchSet, IndirectMeshMetadata, IndirectParametersBuffers,
            IndirectParametersCpuMetadata, InputDataIndex, PhaseItemBatchSetKey,
            PhaseItemExtraIndex,
        },
        tracked_pass::DrawCapabilities,
    },
    tcs::world::World,
};

#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub(super) struct InstancePayload {
    pub(super) transform_id: u32,
    material_id: u32,
    flags: u32,
    extra: u32,
}

impl InstancePayload {
    pub(super) const fn of(transform_id: u32, material_id: u32) -> Self {
        Self {
            transform_id,
            material_id,
            flags: 1,
            extra: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct BatchKey {
    pub(super) group: u32,
    pub(super) indexed: bool,
}

impl PhaseItemBatchSetKey for BatchKey {
    fn indexed(&self) -> bool {
        self.indexed
    }

    fn draw_function(&self) -> DrawFunctionId {
        DrawFunctionId(self.group)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PhaseItemA {
    pub(super) key: BatchKey,
    pub(super) bin: u32,
    rep: BinnedItemId,
    pub(super) range: Range<u32>,
    pub(super) extra: PhaseItemExtraIndex,
}

impl BinnedPhaseItem for PhaseItemA {
    type BatchSetKey = BatchKey;
    type BinKey = u32;

    fn new(
        key: Self::BatchSetKey,
        bin: Self::BinKey,
        rep: BinnedItemId,
        range: Range<u32>,
        extra: PhaseItemExtraIndex,
    ) -> Self {
        Self {
            key,
            bin,
            rep,
            range,
            extra,
        }
    }

    fn batch_range(&self) -> &Range<u32> {
        &self.range
    }

    fn extra_index(&self) -> PhaseItemExtraIndex {
        self.extra.clone()
    }

    fn draw_function(&self) -> DrawFunctionId {
        self.key.draw_function()
    }
}

#[derive(Default)]
pub(super) struct SceneInputs {
    pub(super) rows: Vec<(InstancePayload, Option<IndirectMeshMetadata>)>,
}

pub(super) struct SceneBatcher;

impl GetBatchData<PhaseItemA> for SceneBatcher {
    type BufferData = InstancePayload;

    fn get_batch_data(
        world: &World,
        _item: BinnedItemId,
        input_index: InputDataIndex,
    ) -> Option<(Self::BufferData, Option<IndirectMeshMetadata>)> {
        world
            .resources
            .get::<SceneInputs>()
            .and_then(|s| s.rows.get(input_index.0 as usize).cloned())
    }
}

impl GetFullBatchData<PhaseItemA> for SceneBatcher {
    type BufferInputData = InstancePayload;

    fn get_binned_input_data(
        world: &World,
        item: BinnedItemId,
        input_index: InputDataIndex,
    ) -> Option<Self::BufferInputData> {
        <Self as GetBatchData<PhaseItemA>>::get_batch_data(world, item, input_index).map(|(d, _)| d)
    }

    fn get_binned_indirect_metadata(
        world: &World,
        item: BinnedItemId,
    ) -> Option<IndirectMeshMetadata> {
        world
            .resources
            .get::<SceneInputs>()
            .and_then(|s| s.rows.get(item.0 as usize).and_then(|(_, m)| m.clone()))
    }
}

pub(super) fn setup_multidraw_world() -> World {
    let mut world = World::default();
    let mut inputs = SceneInputs::default();
    for i in 0..2 {
        inputs.rows.push((
            InstancePayload::of(300 + i, 30),
            Some(IndirectMeshMetadata::Indexed {
                index_range: 12..18,
                base_vertex: -4,
            }),
        ));
    }
    inputs.rows.push((
        InstancePayload::of(100, 10),
        Some(IndirectMeshMetadata::Indexed {
            index_range: 0..6,
            base_vertex: 0,
        }),
    ));
    for i in 0..3 {
        inputs.rows.push((
            InstancePayload::of(200 + i, 20),
            Some(IndirectMeshMetadata::Indexed {
                index_range: 6..12,
                base_vertex: 8,
            }),
        ));
    }
    for i in 0..2 {
        inputs.rows.push((
            InstancePayload::of(500 + i, 5),
            Some(IndirectMeshMetadata::NonIndexed {
                vertex_range: 4..10,
            }),
        ));
    }
    world.resources.insert(inputs);
    world
        .resources
        .init::<BatchedInstanceBuffer<InstancePayload>>();
    world.resources.init::<IndirectParametersBuffers>();

    let mut phase = BinnedRenderPhase::<PhaseItemA>::new();
    let indexed_key = BatchKey {
        group: 1,
        indexed: true,
    };
    let non_indexed_key = BatchKey {
        group: 2,
        indexed: false,
    };

    phase.add_instanced(
        indexed_key.clone(),
        30,
        BinnedItemId(0),
        InputDataIndex(0),
        NonZeroU32::new(2).unwrap(),
        BinnedRenderPhaseType::MultidrawableMesh,
    );
    phase.add(
        indexed_key.clone(),
        10,
        BinnedItemId(2),
        InputDataIndex(2),
        BinnedRenderPhaseType::MultidrawableMesh,
    );
    phase.add_instanced(
        indexed_key,
        20,
        BinnedItemId(3),
        InputDataIndex(3),
        NonZeroU32::new(3).unwrap(),
        BinnedRenderPhaseType::MultidrawableMesh,
    );
    phase.add_instanced(
        non_indexed_key,
        5,
        BinnedItemId(6),
        InputDataIndex(6),
        NonZeroU32::new(2).unwrap(),
        BinnedRenderPhaseType::MultidrawableMesh,
    );
    world.resources.insert(phase);
    world
}

#[test]
fn cross_bin_multidraw_indirect_aggregates_exact_args_and_falls_back_cleanly() {
    assert_eq!(std::mem::size_of::<DrawIndirectArgs>(), 16);
    assert_eq!(std::mem::size_of::<DrawIndexedIndirectArgs>(), 20);
    assert_eq!(std::mem::size_of::<IndirectBatchSet>(), 8);
    assert_eq!(std::mem::size_of::<IndirectParametersCpuMetadata>(), 8);

    let mut world = setup_multidraw_world();
    let full_caps = DrawCapabilities {
        base_vertex: true,
        indirect_execution: true,
        indirect_first_instance: true,
        multi_draw_indirect: true,
        multi_draw_indirect_count: true,
        vertex_storage_buffers: true,
    };
    batch_and_prepare_binned_render_phase_cpu::<PhaseItemA, SceneBatcher>(
        &mut world, full_caps, 256,
    );

    let phase = world
        .resources
        .get::<BinnedRenderPhase<PhaseItemA>>()
        .unwrap();
    assert!(matches!(
        phase.batch_sets(),
        BinnedRenderPhaseBatchSets::MultidrawIndirect(_)
    ));
    let sets = phase.batch_sets().as_slice();
    assert_eq!(sets.len(), 2);
    assert_eq!(
        sets[0],
        BinnedRenderPhaseBatchSet::MultidrawIndirect {
            batch_set_key: BatchKey {
                group: 1,
                indexed: true
            },
            bin_key: 10,
            representative_item: BinnedItemId(2),
            instance_range: 0..6,
            indirect_parameters_index: 0,
            batch_count: 3,
            batch_set_index: NonZeroU32::new(1).unwrap(),
        }
    );

    let indirect = world.resources.get::<IndirectParametersBuffers>().unwrap();
    assert_eq!(
        indirect.indexed_data(),
        &[
            DrawIndexedIndirectArgs {
                index_count: 6,
                instance_count: 1,
                first_index: 0,
                base_vertex: 0,
                first_instance: 0
            },
            DrawIndexedIndirectArgs {
                index_count: 6,
                instance_count: 3,
                first_index: 6,
                base_vertex: 8,
                first_instance: 1
            },
            DrawIndexedIndirectArgs {
                index_count: 6,
                instance_count: 2,
                first_index: 12,
                base_vertex: -4,
                first_instance: 4
            },
        ]
    );
    assert_eq!(
        indirect.non_indexed_data(),
        &[DrawIndirectArgs {
            vertex_count: 6,
            instance_count: 2,
            first_vertex: 4,
            first_instance: 6
        }]
    );
    assert_eq!(
        indirect.batch_sets_data(),
        &[
            IndirectBatchSet {
                indirect_parameters_count: 3,
                indirect_parameters_base: 0
            },
            IndirectBatchSet {
                indirect_parameters_count: 1,
                indirect_parameters_base: 0
            },
        ]
    );

    // Fallback 1: multi_draw_indirect = false -> Single batch list with single-batch indirect indices
    let mut world_no_md = setup_multidraw_world();
    batch_and_prepare_binned_render_phase_cpu::<PhaseItemA, SceneBatcher>(
        &mut world_no_md,
        DrawCapabilities {
            multi_draw_indirect: false,
            ..full_caps
        },
        256,
    );
    let phase_no_md = world_no_md
        .resources
        .get::<BinnedRenderPhase<PhaseItemA>>()
        .unwrap();
    assert!(matches!(
        phase_no_md.batch_sets(),
        BinnedRenderPhaseBatchSets::Single(_)
    ));
    match &phase_no_md.batch_sets().as_slice()[0] {
        BinnedRenderPhaseBatchSet::Batches { batches, .. } => {
            assert_eq!(batches.len(), 3);
            assert_eq!((batches[0].0, &batches[0].1.instance_range), (10, &(0..1)));
            assert_eq!(
                batches[0].1.extra_index,
                PhaseItemExtraIndex::IndirectParametersIndex {
                    range: 0..1,
                    batch_set_index: None
                }
            );
            assert_eq!((batches[1].0, &batches[1].1.instance_range), (20, &(1..4)));
            assert_eq!((batches[2].0, &batches[2].1.instance_range), (30, &(4..6)));
        }
        other => panic!("expected Single Batches, got {other:?}"),
    }

    // Fallback 2: indirect_first_instance = false -> direct hardware instancing (PhaseItemExtraIndex::None)
    let mut world_direct = setup_multidraw_world();
    batch_and_prepare_binned_render_phase_cpu::<PhaseItemA, SceneBatcher>(
        &mut world_direct,
        DrawCapabilities {
            indirect_first_instance: false,
            multi_draw_indirect: false,
            ..full_caps
        },
        256,
    );
    let mut emitted = Vec::new();
    world_direct
        .resources
        .get::<BinnedRenderPhase<PhaseItemA>>()
        .unwrap()
        .for_each_render_item(|item| emitted.push(item));
    assert_eq!(emitted.len(), 4);
    assert!(emitted.iter().all(|i| i.extra == PhaseItemExtraIndex::None));
}

#[test]
fn mixed_multidrawable_and_batchable_bins_preserve_strict_bin_key_order() {
    let run_mode = |caps: DrawCapabilities, with_meta_mismatch: bool| -> (Vec<u32>, Vec<u32>) {
        let mut world = World::default();
        let valid_meta = Some(IndirectMeshMetadata::Indexed {
            index_range: 0..6,
            base_vertex: 0,
        });
        let rows = vec![
            (InstancePayload::of(1, 1), valid_meta.clone()),
            (InstancePayload::of(3, 3), valid_meta.clone()),
            (
                InstancePayload::of(5, 5),
                if with_meta_mismatch { None } else { valid_meta },
            ),
        ];
        world.resources.insert(SceneInputs { rows });
        world
            .resources
            .init::<BatchedInstanceBuffer<InstancePayload>>();
        world.resources.init::<IndirectParametersBuffers>();

        let key = BatchKey {
            group: 1,
            indexed: true,
        };
        let mut phase = BinnedRenderPhase::<PhaseItemA>::new();
        phase.add(
            key.clone(),
            5,
            BinnedItemId(50),
            InputDataIndex(2),
            BinnedRenderPhaseType::MultidrawableMesh,
        );
        phase.add(
            key.clone(),
            3,
            BinnedItemId(30),
            InputDataIndex(1),
            BinnedRenderPhaseType::BatchableMesh,
        );
        phase.add(
            key,
            1,
            BinnedItemId(10),
            InputDataIndex(0),
            BinnedRenderPhaseType::MultidrawableMesh,
        );
        world.resources.insert(phase);

        batch_and_prepare_binned_render_phase_cpu::<PhaseItemA, SceneBatcher>(
            &mut world, caps, 256,
        );
        let mut bins = Vec::new();
        world
            .resources
            .get::<BinnedRenderPhase<PhaseItemA>>()
            .unwrap()
            .for_each_render_item(|item| bins.push(item.bin));
        let transforms = world
            .resources
            .get::<BatchedInstanceBuffer<InstancePayload>>()
            .unwrap()
            .instances()
            .iter()
            .map(|i| i.transform_id)
            .collect();
        (bins, transforms)
    };

    let single_caps = DrawCapabilities {
        base_vertex: true,
        indirect_execution: true,
        indirect_first_instance: true,
        multi_draw_indirect: false,
        multi_draw_indirect_count: false,
        vertex_storage_buffers: true,
    };
    assert_eq!(run_mode(single_caps, false), (vec![1, 3, 5], vec![1, 3, 5]));
    assert_eq!(
        run_mode(DrawCapabilities::default(), false),
        (vec![1, 3, 5], vec![1, 3, 5])
    );
    let md_caps = DrawCapabilities {
        multi_draw_indirect: true,
        ..single_caps
    };
    assert_eq!(run_mode(md_caps, true), (vec![1, 3, 5], vec![1, 3, 5]));
}
