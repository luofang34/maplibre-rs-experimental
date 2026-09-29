#![allow(clippy::expect_used, clippy::panic)]

use std::rc::Rc;

use super::super::IntoMessage;
use crate::{
    context::MapContext,
    coords::{WorldCoords, WorldTileCoords, Zoom, ZoomLevel},
    headless::{create_headless_renderer, environment::HeadlessEnvironment},
    io::geometry_index::TileIndex,
    kernel::Kernel,
    render::view_state::ViewState,
    style::Style,
    tcs::{system::System, world::World},
    vector::{transferables::*, DefaultVectorTransferables},
    window::PhysicalSize,
};

async fn setup() -> (Rc<Kernel<HeadlessEnvironment>>, MapContext) {
    let (kernel, renderer) = create_headless_renderer(16, 16, None)
        .await
        .expect("renderer");
    let context = MapContext {
        renderer,
        style: Style::default(),
        world: World::default(),
        view_state: ViewState::new(
            PhysicalSize::new(16, 16).expect("size"),
            WorldCoords::default(),
            Zoom::new(0.0),
            cgmath::Deg(0.0),
            cgmath::Rad(0.64),
        ),
    };
    (Rc::new(kernel), context)
}

#[tokio::test]
async fn symbols_leave_geometry_index_results_for_the_vector_consumer() {
    let (kernel, mut context) = setup().await;
    let message = DefaultLayerIndexed::build_from(
        WorldTileCoords::default(),
        TileIndex::Linear { list: Vec::new() },
    );
    kernel
        .apc()
        .channel
        .0
        .send(IntoMessage::into(message))
        .expect("queue index");
    let mut symbols = crate::sdf::populate_world_system::PopulateWorldSystem::<
        HeadlessEnvironment,
        DefaultVectorTransferables,
    >::new(&kernel);
    symbols
        .run(&mut context)
        .expect("symbols have no matching results");
    let mut vectors = crate::vector::populate_world_system::PopulateWorldSystem::<
        HeadlessEnvironment,
        DefaultVectorTransferables,
    >::new(&kernel);
    vectors.run(&mut context).expect("vector consumes index");
    assert!(context
        .world
        .tiles
        .geometry_index
        .query_point(&WorldCoords::default(), ZoomLevel::new(0), Zoom::new(0.0))
        .is_some());
}
