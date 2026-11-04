use std::{num::NonZeroU32, ops::Range};

use super::{
    setup_multidraw_world, BatchKey, InstancePayload, PhaseItemA, SceneBatcher, SceneInputs,
};
use crate::{
    render::{
        render_phase::{
            batch_and_prepare_binned_render_phase_cpu, BatchedInstanceBuffer, BinnedItemId,
            BinnedPhaseItem, BinnedRenderPhase, BinnedRenderPhaseBatchSets,
            BinnedRenderPhasePlugin, BinnedRenderPhaseType, DrawFunctionId,
            FallbackInstanceBufferMode, GetBatchData, GetFullBatchData, IndirectMeshMetadata,
            InputDataIndex, PhaseItemExtraIndex,
        },
        tracked_pass::DrawCapabilities,
        RenderStageLabel,
    },
    schedule::Schedule,
    tcs::{system::stage::SystemStage, world::World},
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct PhaseItemB(PhaseItemA);

impl BinnedPhaseItem for PhaseItemB {
    type BatchSetKey = BatchKey;
    type BinKey = u32;

    fn new(
        key: Self::BatchSetKey,
        bin: Self::BinKey,
        rep: BinnedItemId,
        range: Range<u32>,
        extra: PhaseItemExtraIndex,
    ) -> Self {
        Self(PhaseItemA::new(key, bin, rep, range, extra))
    }

    fn batch_range(&self) -> &Range<u32> {
        self.0.batch_range()
    }

    fn extra_index(&self) -> PhaseItemExtraIndex {
        self.0.extra_index()
    }

    fn draw_function(&self) -> DrawFunctionId {
        self.0.draw_function()
    }
}

impl GetBatchData<PhaseItemB> for SceneBatcher {
    type BufferData = InstancePayload;

    fn get_batch_data(
        world: &World,
        item: BinnedItemId,
        input_index: InputDataIndex,
    ) -> Option<(Self::BufferData, Option<IndirectMeshMetadata>)> {
        <Self as GetBatchData<PhaseItemA>>::get_batch_data(world, item, input_index)
    }
}

#[test]
fn downlevel_dynamic_uniform_vertex_instance_and_shared_multi_phase_buffers() {
    let mut world = setup_multidraw_world();
    let mut phase_b = BinnedRenderPhase::<PhaseItemB>::new();
    phase_b.add(
        BatchKey {
            group: 9,
            indexed: true,
        },
        1,
        BinnedItemId(2),
        InputDataIndex(2),
        BinnedRenderPhaseType::MultidrawableMesh,
    );
    world.resources.insert(phase_b);

    let full_caps = DrawCapabilities {
        base_vertex: true,
        indirect_execution: true,
        indirect_first_instance: true,
        multi_draw_indirect: true,
        multi_draw_indirect_count: false,
        vertex_storage_buffers: true,
    };
    batch_and_prepare_binned_render_phase_cpu::<PhaseItemA, SceneBatcher>(
        &mut world, full_caps, 256,
    );
    batch_and_prepare_binned_render_phase_cpu::<PhaseItemB, SceneBatcher>(
        &mut world, full_caps, 256,
    );

    let mut b_items = Vec::new();
    world
        .resources
        .get::<BinnedRenderPhase<PhaseItemB>>()
        .unwrap()
        .for_each_render_item(|i| b_items.push(i));
    assert_eq!(b_items.len(), 1);
    assert_eq!(b_items[0].0.range, 8..9);
    assert_eq!(
        b_items[0].0.extra,
        PhaseItemExtraIndex::IndirectParametersIndex {
            range: 3..4,
            batch_set_index: NonZeroU32::new(3)
        }
    );

    // WebGL2 DynamicUniform fallback (vertex_storage_buffers = false, alignment = 256)
    let mut webgl_world = setup_multidraw_world();
    batch_and_prepare_binned_render_phase_cpu::<PhaseItemA, SceneBatcher>(
        &mut webgl_world,
        DrawCapabilities::default(),
        256,
    );
    let webgl_phase = webgl_world
        .resources
        .get::<BinnedRenderPhase<PhaseItemA>>()
        .unwrap();
    assert!(matches!(
        webgl_phase.batch_sets(),
        BinnedRenderPhaseBatchSets::DynamicUniforms(_)
    ));
    let mut dyn_items = Vec::new();
    webgl_phase.for_each_render_item(|i| dyn_items.push(i));
    assert_eq!(dyn_items.len(), 8);
    for (idx, item) in dyn_items.iter().enumerate() {
        assert_eq!(
            (&item.range, &item.extra),
            (
                &(0..1),
                &PhaseItemExtraIndex::DynamicOffset((idx as u32) * 256)
            )
        );
    }
    let ib = webgl_world
        .resources
        .get::<BatchedInstanceBuffer<InstancePayload>>()
        .unwrap();
    assert_eq!(
        (
            ib.aligned_uniform_stride(),
            ib.aligned_uniform_bytes().len()
        ),
        (256, 8 * 256)
    );

    // WebGL2 VertexInstance fallback (vertex_storage_buffers = false, FallbackInstanceBufferMode::VertexInstance)
    let mut vstep_world = setup_multidraw_world();
    vstep_world
        .resources
        .get_mut::<BatchedInstanceBuffer<InstancePayload>>()
        .unwrap()
        .set_fallback_mode(FallbackInstanceBufferMode::VertexInstance);
    batch_and_prepare_binned_render_phase_cpu::<PhaseItemA, SceneBatcher>(
        &mut vstep_world,
        DrawCapabilities::default(),
        256,
    );
    let vstep_phase = vstep_world
        .resources
        .get::<BinnedRenderPhase<PhaseItemA>>()
        .unwrap();
    assert!(matches!(
        vstep_phase.batch_sets(),
        BinnedRenderPhaseBatchSets::Single(_)
    ));
    let mut vstep_items = Vec::new();
    vstep_phase.for_each_render_item(|i| vstep_items.push(i));
    assert_eq!(vstep_items.len(), 4);
    assert!(vstep_items
        .iter()
        .all(|i| i.extra == PhaseItemExtraIndex::VertexInstance));
    assert_eq!(
        (
            &vstep_items[0].range,
            &vstep_items[1].range,
            &vstep_items[2].range,
            &vstep_items[3].range
        ),
        (&(0..1), &(1..4), &(4..6), &(6..8))
    );
    assert_eq!(
        SceneBatcher::get_binned_input_data(&vstep_world, BinnedItemId(2), InputDataIndex(2))
            .unwrap()
            .transform_id,
        100
    );
    assert!(
        SceneBatcher::get_binned_indirect_metadata(&vstep_world, BinnedItemId(2))
            .unwrap()
            .is_indexed()
    );
}

#[test]
fn dual_phases_sharing_instance_buffer_preserve_distinct_downlevel_fallback_modes() {
    let mut world = World::default();
    world.resources.insert(SceneInputs {
        rows: (0..6u32)
            .map(|i| (InstancePayload::of(100 + i, i), None))
            .collect(),
    });

    let mut schedule = Schedule::default();
    for stage in [
        RenderStageLabel::Prepare,
        RenderStageLabel::Queue,
        RenderStageLabel::PhaseSort,
        RenderStageLabel::Render,
        RenderStageLabel::Cleanup,
    ] {
        schedule.add_stage(stage, SystemStage::default()).unwrap();
    }

    BinnedRenderPhasePlugin::<PhaseItemA, SceneBatcher>::new()
        .with_fallback_mode(FallbackInstanceBufferMode::DynamicUniform)
        .install(&mut schedule, &mut world)
        .unwrap();
    BinnedRenderPhasePlugin::<PhaseItemB, SceneBatcher>::new()
        .with_fallback_mode(FallbackInstanceBufferMode::VertexInstance)
        .install(&mut schedule, &mut world)
        .unwrap();

    assert_eq!(
        world
            .resources
            .get::<BinnedRenderPhase<PhaseItemA>>()
            .unwrap()
            .fallback_mode(),
        Some(FallbackInstanceBufferMode::DynamicUniform)
    );
    assert_eq!(
        world
            .resources
            .get::<BinnedRenderPhase<PhaseItemB>>()
            .unwrap()
            .fallback_mode(),
        Some(FallbackInstanceBufferMode::VertexInstance)
    );

    world
        .resources
        .get_mut::<BinnedRenderPhase<PhaseItemA>>()
        .unwrap()
        .add_instanced(
            BatchKey {
                group: 1,
                indexed: true,
            },
            10,
            BinnedItemId(0),
            InputDataIndex(0),
            NonZeroU32::new(2).unwrap(),
            BinnedRenderPhaseType::BatchableMesh,
        );
    world
        .resources
        .get_mut::<BinnedRenderPhase<PhaseItemB>>()
        .unwrap()
        .add_instanced(
            BatchKey {
                group: 2,
                indexed: true,
            },
            20,
            BinnedItemId(3),
            InputDataIndex(3),
            NonZeroU32::new(3).unwrap(),
            BinnedRenderPhaseType::BatchableMesh,
        );

    batch_and_prepare_binned_render_phase_cpu::<PhaseItemA, SceneBatcher>(
        &mut world,
        DrawCapabilities::default(),
        256,
    );
    batch_and_prepare_binned_render_phase_cpu::<PhaseItemB, SceneBatcher>(
        &mut world,
        DrawCapabilities::default(),
        256,
    );

    let mut a_items = Vec::new();
    world
        .resources
        .get::<BinnedRenderPhase<PhaseItemA>>()
        .unwrap()
        .for_each_render_item(|i| a_items.push(i));
    assert_eq!(a_items.len(), 2);
    assert_eq!(
        (&a_items[0].extra, &a_items[1].extra),
        (
            &PhaseItemExtraIndex::DynamicOffset(0),
            &PhaseItemExtraIndex::DynamicOffset(256)
        )
    );

    let mut b_items = Vec::new();
    world
        .resources
        .get::<BinnedRenderPhase<PhaseItemB>>()
        .unwrap()
        .for_each_render_item(|i| b_items.push(i));
    assert_eq!(b_items.len(), 1);
    assert_eq!(b_items[0].0.range, 2..5);
    assert_eq!(b_items[0].0.extra, PhaseItemExtraIndex::VertexInstance);

    let ib = world
        .resources
        .get::<BatchedInstanceBuffer<InstancePayload>>()
        .unwrap();
    assert_eq!(ib.aligned_uniform_bytes().len(), 2 * 256);
    assert_eq!(ib.instances().len(), 5);
}
