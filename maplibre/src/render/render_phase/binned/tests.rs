mod sweep;

use std::{num::NonZeroU32, ops::Range};

use bytemuck_derive::{Pod, Zeroable};

use crate::{
    render::{
        render_phase::{
            batch_and_prepare_binned_render_phase_cpu, BatchedInstanceBuffer, BinnedItemId,
            BinnedPhaseItem, BinnedRenderPhase, BinnedRenderPhaseType, Draw, DrawFunctionId,
            DrawFunctions, GetBatchData, IndirectMeshMetadata, IndirectParametersBuffers,
            InputDataIndex, PhaseItemBatchSetKey, PhaseItemExtraIndex, RenderCommand,
            RenderCommandResult,
        },
        tracked_pass::{DrawCapabilities, TrackedRenderPass},
    },
    tcs::world::World,
};

#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub(super) struct TestInstance {
    item_id: u32,
    slot: u32,
    pad: [u32; 2],
}

#[derive(Default)]
pub(super) struct InputTable(pub(super) Vec<(TestInstance, Option<IndirectMeshMetadata>)>);

impl TestInstance {
    pub(super) const fn of(item_id: u32, slot: u32) -> Self {
        Self {
            item_id,
            slot,
            pad: [0; 2],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct TestBatchSetKey {
    pipeline: u32,
    indexed: bool,
    draw_fn: DrawFunctionId,
}

impl TestBatchSetKey {
    pub(super) const fn indexed(pipeline: u32, draw_fn: u32) -> Self {
        Self {
            pipeline,
            indexed: true,
            draw_fn: DrawFunctionId(draw_fn),
        }
    }
}

impl PhaseItemBatchSetKey for TestBatchSetKey {
    fn indexed(&self) -> bool {
        self.indexed
    }

    fn draw_function(&self) -> DrawFunctionId {
        self.draw_fn
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TestBinnedItem {
    batch_set_key: TestBatchSetKey,
    bin_key: u32,
    representative_item: BinnedItemId,
    batch_range: Range<u32>,
    extra_index: PhaseItemExtraIndex,
}

impl BinnedPhaseItem for TestBinnedItem {
    type BatchSetKey = TestBatchSetKey;
    type BinKey = u32;

    fn new(
        batch_set_key: Self::BatchSetKey,
        bin_key: Self::BinKey,
        representative_item: BinnedItemId,
        batch_range: Range<u32>,
        extra_index: PhaseItemExtraIndex,
    ) -> Self {
        Self {
            batch_set_key,
            bin_key,
            representative_item,
            batch_range,
            extra_index,
        }
    }

    fn batch_range(&self) -> &Range<u32> {
        &self.batch_range
    }

    fn extra_index(&self) -> PhaseItemExtraIndex {
        self.extra_index.clone()
    }

    fn draw_function(&self) -> DrawFunctionId {
        self.batch_set_key.draw_fn
    }
}

pub(super) struct TestBatcher;

impl GetBatchData<TestBinnedItem> for TestBatcher {
    type BufferData = TestInstance;

    fn get_batch_data(
        world: &World,
        _item: BinnedItemId,
        input_index: InputDataIndex,
    ) -> Option<(Self::BufferData, Option<IndirectMeshMetadata>)> {
        world
            .resources
            .get::<InputTable>()
            .and_then(|t| t.0.get(input_index.0 as usize).cloned())
    }
}

struct CmdSuccessA;
struct CmdSuccessB;
struct CmdFail;

impl RenderCommand<TestBinnedItem> for CmdSuccessA {
    fn render<'w>(
        _world: &'w World,
        _item: &TestBinnedItem,
        _pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        RenderCommandResult::Success
    }
}

impl RenderCommand<TestBinnedItem> for CmdSuccessB {
    fn render<'w>(
        _world: &'w World,
        _item: &TestBinnedItem,
        _pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        RenderCommandResult::Success
    }
}

impl RenderCommand<TestBinnedItem> for CmdFail {
    fn render<'w>(
        _world: &'w World,
        _item: &TestBinnedItem,
        _pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        RenderCommandResult::Failure
    }
}

struct CustomDraw;

impl Draw<TestBinnedItem> for CustomDraw {
    fn draw<'w>(
        &self,
        _pass: &mut TrackedRenderPass<'w>,
        _world: &'w World,
        _item: &TestBinnedItem,
    ) {
    }
}

#[test]
fn draw_functions_idempotent_registration_and_lookup() {
    let mut registry = DrawFunctions::<TestBinnedItem>::new();
    assert!(registry.is_empty());

    let id_ab_1 = registry.add_with::<(CmdSuccessA, CmdSuccessB)>();
    let id_ab_2 = registry.add_with::<(CmdSuccessA, CmdSuccessB)>();
    assert_eq!(id_ab_1, DrawFunctionId(0));
    assert_eq!(id_ab_1, id_ab_2);
    assert_eq!(registry.len(), 1);
    assert_eq!(registry.id::<(CmdSuccessA, CmdSuccessB)>(), Some(id_ab_1));

    let id_fail = registry.add_with::<(CmdSuccessA, CmdFail, CmdSuccessB)>();
    assert_eq!(id_fail, DrawFunctionId(1));
    assert_eq!(registry.len(), 2);

    let id_custom_1 = registry.add(CustomDraw);
    let id_custom_2 = registry.add(CustomDraw);
    assert_eq!(id_custom_1, DrawFunctionId(2));
    assert_eq!(id_custom_1, id_custom_2);
    assert_eq!(registry.len(), 3);
    assert!(registry.get(id_ab_1).is_some());
    assert!(registry.get(DrawFunctionId(99)).is_none());
}

#[test]
fn same_bin_coalescing_merges_single_and_preaggregated_instances() {
    let mut world = World::default();
    let table = InputTable(
        (0..8u32)
            .map(|s| (TestInstance::of(100 + s, s), None))
            .collect(),
    );
    world.resources.insert(table);
    world
        .resources
        .init::<BatchedInstanceBuffer<TestInstance>>();
    world.resources.init::<IndirectParametersBuffers>();

    let bsk = TestBatchSetKey::indexed(1, 0);
    let mut phase = BinnedRenderPhase::<TestBinnedItem>::new();
    let nz3 = NonZeroU32::new(3).unwrap();
    let nz2 = NonZeroU32::new(2).unwrap();

    phase.add(
        bsk.clone(),
        20,
        BinnedItemId(1),
        InputDataIndex(0),
        BinnedRenderPhaseType::BatchableMesh,
    );
    phase.add(
        bsk.clone(),
        10,
        BinnedItemId(2),
        InputDataIndex(1),
        BinnedRenderPhaseType::BatchableMesh,
    );
    phase.add_instanced(
        bsk.clone(),
        20,
        BinnedItemId(3),
        InputDataIndex(2),
        nz3,
        BinnedRenderPhaseType::BatchableMesh,
    );
    phase.add_instanced(
        bsk,
        10,
        BinnedItemId(4),
        InputDataIndex(5),
        nz2,
        BinnedRenderPhaseType::BatchableMesh,
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
        .get::<BinnedRenderPhase<TestBinnedItem>>()
        .unwrap();
    let mut rendered = Vec::new();
    phase.for_each_render_item(|item| rendered.push(item));

    assert_eq!(rendered.len(), 2);
    assert_eq!(
        (
            rendered[0].bin_key,
            rendered[0].representative_item,
            &rendered[0].batch_range
        ),
        (10, BinnedItemId(2), &(0..3))
    );
    assert_eq!(
        (
            rendered[1].bin_key,
            rendered[1].representative_item,
            &rendered[1].batch_range
        ),
        (20, BinnedItemId(1), &(3..7))
    );

    let ib = world
        .resources
        .get::<BatchedInstanceBuffer<TestInstance>>()
        .unwrap();
    let slots: Vec<u32> = ib.instances().iter().map(|i| i.slot).collect();
    assert_eq!(slots, vec![1, 5, 6, 0, 2, 3, 4]);
}

#[test]
fn four_way_routing_produces_deterministic_order_and_exact_ranges() {
    let build_and_collect = |reverse: bool| -> Vec<TestBinnedItem> {
        let mut world = World::default();
        let meta = Some(IndirectMeshMetadata::Indexed {
            index_range: 0..6,
            base_vertex: 0,
        });
        world.resources.insert(InputTable(
            (0..6u32)
                .map(|s| (TestInstance::of(s, s), meta.clone()))
                .collect(),
        ));
        world
            .resources
            .init::<BatchedInstanceBuffer<TestInstance>>();
        world.resources.init::<IndirectParametersBuffers>();

        let bsk_a = TestBatchSetKey::indexed(1, 0);
        let bsk_b = TestBatchSetKey::indexed(2, 1);
        let mut phase = BinnedRenderPhase::<TestBinnedItem>::new();
        let steps = [0usize, 1, 2, 3, 4];
        let order: Vec<usize> = if reverse {
            steps.into_iter().rev().collect()
        } else {
            steps.into_iter().collect()
        };

        for step in order {
            match step {
                0 => phase.add(
                    bsk_a.clone(),
                    5,
                    BinnedItemId(10),
                    InputDataIndex(0),
                    BinnedRenderPhaseType::MultidrawableMesh,
                ),
                1 => phase.add(
                    bsk_a.clone(),
                    2,
                    BinnedItemId(11),
                    InputDataIndex(1),
                    BinnedRenderPhaseType::MultidrawableMesh,
                ),
                2 => phase.add(
                    bsk_b.clone(),
                    1,
                    BinnedItemId(20),
                    InputDataIndex(2),
                    BinnedRenderPhaseType::BatchableMesh,
                ),
                3 => phase.add_with_extra_index(
                    bsk_a.clone(),
                    9,
                    BinnedItemId(30),
                    InputDataIndex(3),
                    NonZeroU32::MIN,
                    PhaseItemExtraIndex::DynamicOffset(512),
                    BinnedRenderPhaseType::UnbatchableMesh,
                ),
                _ => phase.add_instanced(
                    bsk_b.clone(),
                    4,
                    BinnedItemId(40),
                    InputDataIndex(99),
                    NonZeroU32::new(2).unwrap(),
                    BinnedRenderPhaseType::NonMesh,
                ),
            }
        }
        world.resources.insert(phase);

        let caps = DrawCapabilities {
            base_vertex: true,
            indirect_execution: true,
            indirect_first_instance: true,
            multi_draw_indirect: true,
            multi_draw_indirect_count: false,
            vertex_storage_buffers: true,
        };
        batch_and_prepare_binned_render_phase_cpu::<TestBinnedItem, TestBatcher>(
            &mut world, caps, 256,
        );

        let mut items = Vec::new();
        world
            .resources
            .get::<BinnedRenderPhase<TestBinnedItem>>()
            .unwrap()
            .for_each_render_item(|i| items.push(i));
        items
    };

    let forward = build_and_collect(false);
    let reverse = build_and_collect(true);
    assert_eq!(forward, reverse);
    assert_eq!(forward.len(), 4);

    assert_eq!(
        (
            forward[0].batch_set_key.pipeline,
            forward[0].bin_key,
            &forward[0].batch_range
        ),
        (1, 2, &(0..2))
    );
    assert_eq!(
        forward[0].extra_index,
        PhaseItemExtraIndex::IndirectParametersIndex {
            range: 0..2,
            batch_set_index: NonZeroU32::new(1)
        }
    );
    assert_eq!(
        (
            forward[1].batch_set_key.pipeline,
            forward[1].bin_key,
            &forward[1].batch_range
        ),
        (2, 1, &(2..3))
    );
    assert_eq!(
        forward[1].extra_index,
        PhaseItemExtraIndex::IndirectParametersIndex {
            range: 2..3,
            batch_set_index: None
        }
    );
    assert_eq!(
        (
            forward[2].representative_item,
            &forward[2].batch_range,
            &forward[2].extra_index
        ),
        (
            BinnedItemId(30),
            &(0..1),
            &PhaseItemExtraIndex::DynamicOffset(512)
        )
    );
    assert_eq!(
        (
            forward[3].representative_item,
            &forward[3].batch_range,
            &forward[3].extra_index
        ),
        (BinnedItemId(40), &(0..2), &PhaseItemExtraIndex::None)
    );
}
