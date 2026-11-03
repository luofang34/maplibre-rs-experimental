use std::num::NonZeroU32;

use super::{
    setup_multidraw_world, BatchKey, InstancePayload, PhaseItemA, PhaseItemB, SceneBatcher,
};
use crate::render::{
    render_phase::{
        batch_and_prepare_binned_render_phase_cpu, BatchedInstanceBuffer, BinnedItemId,
        BinnedRenderPhase, BinnedRenderPhaseBatchSets, BinnedRenderPhaseType,
        FallbackInstanceBufferMode, GetFullBatchData, InputDataIndex, PhaseItemExtraIndex,
    },
    tracked_pass::DrawCapabilities,
};

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
