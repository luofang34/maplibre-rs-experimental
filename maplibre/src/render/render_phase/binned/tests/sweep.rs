use std::num::NonZeroU32;

use super::{InputTable, TestBatchSetKey, TestBatcher, TestBinnedItem, TestInstance};
use crate::{
    render::{
        eye_covering::EyeInFrame,
        render_phase::{
            batch_and_prepare_binned_render_phase_cpu,
            binned::DEFAULT_MAX_INACTIVE_GENERATIONS,
            plugin::{cleanup_binned_phase_world, prepare_binned_phase_world},
            BatchedInstanceBuffer, BinnedItemId, BinnedRenderPhase, BinnedRenderPhaseType,
            IndirectParametersBuffers, InputDataIndex,
        },
        tracked_pass::DrawCapabilities,
    },
    tcs::world::World,
};

#[test]
fn sweep_retains_warm_capacity_evicts_cold_bins_and_protects_multi_view() {
    assert_eq!(DEFAULT_MAX_INACTIVE_GENERATIONS, 16);

    let bsk = TestBatchSetKey::indexed(1, 0);
    let mut phase = BinnedRenderPhase::<TestBinnedItem>::new().with_max_inactive_generations(2);

    for i in 0..16 {
        phase.add(
            bsk.clone(),
            1,
            BinnedItemId(i),
            InputDataIndex(i as u32),
            BinnedRenderPhaseType::BatchableMesh,
        );
    }
    for i in 0..4 {
        phase.add(
            bsk.clone(),
            2,
            BinnedItemId(100 + i),
            InputDataIndex(i as u32),
            BinnedRenderPhaseType::BatchableMesh,
        );
    }
    assert!(!phase.is_empty());
    phase.sweep();
    assert!(phase.is_empty());
    assert_eq!((phase.generation(), phase.allocated_bin_count()), (1, 2));
    let cap_frame1 = phase
        .bin_capacity(&bsk, &1, BinnedRenderPhaseType::BatchableMesh)
        .unwrap();
    assert!(cap_frame1 >= 16);

    for expected_gen in 2..=4 {
        for i in 0..8 {
            phase.add(
                bsk.clone(),
                1,
                BinnedItemId(i),
                InputDataIndex(i as u32),
                BinnedRenderPhaseType::BatchableMesh,
            );
        }
        assert_eq!(
            phase.bin_capacity(&bsk, &1, BinnedRenderPhaseType::BatchableMesh),
            Some(cap_frame1)
        );
        phase.sweep();
        assert_eq!(phase.generation(), expected_gen);
    }
    assert_eq!(phase.allocated_bin_count(), 1);
    assert_eq!(
        phase.bin_capacity(&bsk, &1, BinnedRenderPhaseType::BatchableMesh),
        Some(cap_frame1)
    );
    assert_eq!(
        phase.bin_capacity(&bsk, &2, BinnedRenderPhaseType::BatchableMesh),
        None
    );

    assert_eq!(
        BinnedRenderPhaseType::mesh(true, true),
        BinnedRenderPhaseType::MultidrawableMesh
    );
    assert_eq!(
        BinnedRenderPhaseType::mesh(true, false),
        BinnedRenderPhaseType::BatchableMesh
    );
    assert_eq!(
        BinnedRenderPhaseType::mesh(false, true),
        BinnedRenderPhaseType::UnbatchableMesh
    );
}

#[test]
fn xr_frame_zero_stereo_generations_match_and_lingering_eye_in_frame_sweeps_every_frame() {
    let bsk = TestBatchSetKey::indexed(1, 0);
    let mut world = World::default();
    world
        .resources
        .insert(BinnedRenderPhase::<TestBinnedItem>::new().with_max_inactive_generations(2));
    world
        .resources
        .init::<BatchedInstanceBuffer<TestInstance>>();
    world.resources.init::<IndirectParametersBuffers>();

    // XR stereo frame 0: Eye 0 queues bin 101, then Eye 1 queues bin 202.
    world.resources.insert(EyeInFrame { index: 0, frame: 0 });
    prepare_binned_phase_world::<TestBinnedItem, TestBatcher>(&mut world).unwrap();
    world
        .resources
        .get_mut::<BinnedRenderPhase<TestBinnedItem>>()
        .unwrap()
        .add(
            bsk.clone(),
            101,
            BinnedItemId(1),
            InputDataIndex(0),
            BinnedRenderPhaseType::BatchableMesh,
        );
    cleanup_binned_phase_world::<TestBinnedItem>(&mut world).unwrap();

    world.resources.insert(EyeInFrame { index: 1, frame: 0 });
    prepare_binned_phase_world::<TestBinnedItem, TestBatcher>(&mut world).unwrap();
    world
        .resources
        .get_mut::<BinnedRenderPhase<TestBinnedItem>>()
        .unwrap()
        .add(
            bsk.clone(),
            202,
            BinnedItemId(2),
            InputDataIndex(0),
            BinnedRenderPhaseType::BatchableMesh,
        );
    cleanup_binned_phase_world::<TestBinnedItem>(&mut world).unwrap();

    {
        let p = world
            .resources
            .get::<BinnedRenderPhase<TestBinnedItem>>()
            .unwrap();
        assert!(p.is_empty());
        assert_eq!(p.generation(), 1);
        assert_eq!(
            p.bin_last_active_generation(&bsk, &101, BinnedRenderPhaseType::BatchableMesh),
            Some(0)
        );
        assert_eq!(
            p.bin_last_active_generation(&bsk, &202, BinnedRenderPhaseType::BatchableMesh),
            Some(0)
        );
    }

    // Leave lingering EyeInFrame { index: 0, frame: 5 } (just as run_xr_frame / run_multi_view_frame do)
    // and run normal frames without updating EyeInFrame: generation MUST advance every frame and
    // evict both Eye 0-only and Eye 1-only cold bins on the exact same sweep!
    world.resources.insert(EyeInFrame { index: 0, frame: 5 });

    for expected_gen in 2..=3 {
        prepare_binned_phase_world::<TestBinnedItem, TestBatcher>(&mut world).unwrap();
        cleanup_binned_phase_world::<TestBinnedItem>(&mut world).unwrap();
        let p = world
            .resources
            .get::<BinnedRenderPhase<TestBinnedItem>>()
            .unwrap();
        assert_eq!(p.generation(), expected_gen);
        assert_eq!(p.allocated_bin_count(), 2);
    }

    // On the 3rd inactive frame (exceeding max_inactive_generations = 2), both bins are evicted together.
    prepare_binned_phase_world::<TestBinnedItem, TestBatcher>(&mut world).unwrap();
    cleanup_binned_phase_world::<TestBinnedItem>(&mut world).unwrap();
    let p = world
        .resources
        .get::<BinnedRenderPhase<TestBinnedItem>>()
        .unwrap();
    assert_eq!(p.generation(), 4);
    assert_eq!(p.allocated_bin_count(), 0);
}

#[test]
fn unbatchable_and_non_mesh_store_keys_once_per_bin_and_retain_capacity() {
    let bsk = TestBatchSetKey::indexed(1, 0);
    let mut world = World::default();
    world.resources.insert(InputTable(
        (0..6u32).map(|s| (TestInstance::of(s, s), None)).collect(),
    ));
    world
        .resources
        .init::<BatchedInstanceBuffer<TestInstance>>();
    world.resources.init::<IndirectParametersBuffers>();

    let mut phase = BinnedRenderPhase::<TestBinnedItem>::new();
    for i in 0..4u32 {
        phase.add(
            bsk.clone(),
            7,
            BinnedItemId(i as u64),
            InputDataIndex(i),
            BinnedRenderPhaseType::UnbatchableMesh,
        );
    }
    phase.add_instanced(
        bsk,
        9,
        BinnedItemId(10),
        InputDataIndex(4),
        NonZeroU32::new(2).unwrap(),
        BinnedRenderPhaseType::NonMesh,
    );
    world.resources.insert(phase);

    let caps = DrawCapabilities {
        base_vertex: true,
        vertex_storage_buffers: true,
        ..Default::default()
    };
    batch_and_prepare_binned_render_phase_cpu::<TestBinnedItem, TestBatcher>(&mut world, caps, 256);

    let phase = world
        .resources
        .get_mut::<BinnedRenderPhase<TestBinnedItem>>()
        .unwrap();
    assert_eq!(phase.prepared_unbatchable.groups.len(), 1);
    assert_eq!(phase.prepared_unbatchable.entries.len(), 4);
    assert_eq!(phase.prepared_non_mesh.groups.len(), 1);
    assert_eq!(phase.prepared_non_mesh.entries.len(), 1);

    let entries_cap = phase.prepared_unbatchable.entries.capacity();
    let groups_cap = phase.prepared_unbatchable.groups.capacity();
    phase.clear_entries();
    assert!(phase.prepared_unbatchable.is_empty());
    assert_eq!(phase.prepared_unbatchable.entries.capacity(), entries_cap);
    assert_eq!(phase.prepared_unbatchable.groups.capacity(), groups_cap);
}
