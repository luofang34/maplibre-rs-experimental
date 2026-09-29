use super::{
    fixture::{style, TileKernel},
    *,
};
use crate::{
    context::MapContext,
    coords::ZoomLevel,
    headless::environment::HeadlessEnvironment,
    io::apc::SchedulerContext,
    kernel::Kernel,
    plugin::Plugin,
    render::{graph::RenderGraph, RenderStageLabel},
    schedule::Schedule,
    sdf::SymbolLayersDataComponent,
    tcs::system::{stage::SystemStage, System},
    vector::{
        populate_world_system::PopulateWorldSystem, VectorLayerBucketComponent, VectorPlugin,
    },
};

async fn setup_covering() -> (
    std::rc::Rc<Kernel<HeadlessEnvironment>>,
    MapContext,
    WorldTileCoords,
) {
    let (kernel, mut context) = super::super::systems::setup().await;
    let mut schedule = Schedule::default();
    for stage in [
        RenderStageLabel::Extract,
        RenderStageLabel::Prepare,
        RenderStageLabel::Queue,
    ] {
        schedule.add_stage(stage, SystemStage::default());
    }
    VectorPlugin::<DefaultVectorTransferables>::default().build(
        &mut schedule,
        kernel.clone(),
        &mut context.world,
        &mut RenderGraph::default(),
    );
    let parent = WorldTileCoords::default();
    let child = WorldTileCoords {
        x: 0,
        y: 0,
        z: ZoomLevel::new(1),
    };
    let parent_data = VectorLayerBucketComponent {
        done: true,
        ..Default::default()
    };
    context
        .world
        .tiles
        .spawn_mut(parent)
        .expect("parent")
        .insert(parent_data);
    context
        .world
        .tiles
        .spawn_mut(child)
        .expect("child")
        .insert(VectorLayerBucketComponent::default())
        .insert(SymbolLayersDataComponent::default());
    assert_eq!(request_budget(&context.world), MAX_TILES_IN_FLIGHT - 1);
    (kernel, context, child)
}

async fn selected_source(path: &str) -> (WorldTileCoords, WorldTileCoords) {
    let (kernel, mut context, child) = setup_covering().await;
    crate::vector::request_system::fetch_vector_apc::<_, DefaultVectorTransferables, _>(
        Input::TileRequest {
            coords: child,
            style: style(false, &[path]),
        },
        SchedulerContext {
            sender: kernel.apc().channel.0.clone(),
        },
        TileKernel::default(),
    )
    .await
    .expect("worker reports terminal result");
    PopulateWorldSystem::<HeadlessEnvironment, DefaultVectorTransferables>::new(&kernel)
        .run(&mut context)
        .expect("completion consumed");
    assert_eq!(request_budget(&context.world), MAX_TILES_IN_FLIGHT);
    (covering_source(&context.world, child), child)
}

#[tokio::test]
async fn corrupt_vector_child_releases_capacity_and_retains_parent_drape() {
    assert_eq!(selected_source("bad").await.0, WorldTileCoords::default());
}

#[tokio::test]
async fn unavailable_vector_child_releases_capacity_and_retains_parent_drape() {
    assert_eq!(
        selected_source("missing").await.0,
        WorldTileCoords::default()
    );
}

#[tokio::test]
async fn valid_empty_vector_child_replaces_parent_drape() {
    let (selected, child) = selected_source("empty").await;
    assert_eq!(selected, child);
}

fn covering_source(world: &World, child: WorldTileCoords) -> WorldTileCoords {
    let selected =
        crate::terrain::drape_targets::select_targets(std::iter::once(child), world, &Vec::new());
    let shapes = &selected[0].1;
    assert_eq!(shapes.len(), 1);
    shapes[0].coords
}

async fn coverage_while_a_later_source_loads(first: &str, expected: WorldTileCoords) {
    let (kernel, mut context, child) = setup_covering().await;
    let gate = std::sync::Arc::new(super::fixture::FetchGate::default());
    let client = super::fixture::TileClient {
        gate: Some(gate.clone()),
    };
    let worker = crate::vector::request_system::fetch_vector_apc::<_, DefaultVectorTransferables, _>(
        Input::TileRequest {
            coords: child,
            style: style(false, &[first, "blocked"]),
        },
        SchedulerContext {
            sender: kernel.apc().channel.0.clone(),
        },
        TileKernel(client),
    );
    let intermediate = async {
        gate.entered.notified().await;
        PopulateWorldSystem::<HeadlessEnvironment, DefaultVectorTransferables>::new(&kernel)
            .run(&mut context)
            .expect("intermediate results");
        assert_eq!(request_budget(&context.world), MAX_TILES_IN_FLIGHT - 1);
        assert_eq!(
            covering_source(&context.world, child),
            WorldTileCoords::default()
        );
        gate.resume.notify_one();
    };
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(worker, intermediate)
    })
    .await
    .expect("worker and observer complete");
    result.expect("remaining source processed");
    PopulateWorldSystem::<HeadlessEnvironment, DefaultVectorTransferables>::new(&kernel)
        .run(&mut context)
        .expect("terminal results");
    assert_eq!(request_budget(&context.world), MAX_TILES_IN_FLIGHT);
    assert_eq!(covering_source(&context.world, child), expected);
}

#[tokio::test]
async fn failed_source_retains_parent_while_healthy_source_is_loading() {
    coverage_while_a_later_source_loads("bad", WorldTileCoords::default()).await;
}

#[tokio::test]
async fn partial_multi_source_success_retains_parent_until_all_base_data_arrives() {
    coverage_while_a_later_source_loads("vector", WorldTileCoords::from((0, 0, 1_u8.into()))).await;
}
