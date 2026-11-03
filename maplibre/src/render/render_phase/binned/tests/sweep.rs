use super::{TestBatchSetKey, TestBatcher, TestBinnedItem, TestInstance};
use crate::{
    render::{
        eye_covering::EyeInFrame,
        render_phase::{
            plugin::{cleanup_binned_phase_world, prepare_binned_phase_world},
            BatchedInstanceBuffer, BinnedItemId, BinnedRenderPhase, BinnedRenderPhaseType,
            IndirectParametersBuffers, InputDataIndex,
        },
    },
    tcs::world::World,
};

#[test]
fn sweep_retains_warm_capacity_evicts_cold_bins_and_protects_multi_view() {
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

    // Multi-viewport same-frame EyeInFrame protection: Eye 0 sweeps once, Eye 1 clears items without sweeping Eye 0's bin.
    let mut world = World::default();
    world
        .resources
        .insert(BinnedRenderPhase::<TestBinnedItem>::new().with_max_inactive_generations(0));
    world
        .resources
        .init::<BatchedInstanceBuffer<TestInstance>>();
    world.resources.init::<IndirectParametersBuffers>();

    world.resources.insert(EyeInFrame { index: 0, frame: 5 });
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

    // Eye 1 in the same host frame (frame: 5, index: 1): even with max_inactive_generations = 0, Eye 0's bin 101 must NOT be evicted!
    world.resources.insert(EyeInFrame { index: 1, frame: 5 });
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

    let p = world
        .resources
        .get::<BinnedRenderPhase<TestBinnedItem>>()
        .unwrap();
    assert!(p.is_empty());
    assert_eq!(p.generation(), 1);
    assert!(p
        .bin_capacity(&bsk, &101, BinnedRenderPhaseType::BatchableMesh)
        .is_some());
    assert!(p
        .bin_capacity(&bsk, &202, BinnedRenderPhaseType::BatchableMesh)
        .is_some());

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
