#![allow(clippy::expect_used, clippy::panic)]

use std::rc::Rc;

use super::super::{IntoMessage, Message};
use crate::{
    context::MapContext,
    coords::{WorldCoords, WorldTileCoords, Zoom, ZoomLevel},
    headless::{create_headless_renderer, environment::HeadlessEnvironment},
    io::geometry_index::TileIndex,
    kernel::Kernel,
    render::view_state::ViewState,
    style::Style,
    tcs::{
        system::{System, SystemError},
        world::World,
    },
    vector::{transferables::*, DefaultVectorTransferables, VectorLayerBucketComponent},
    window::PhysicalSize,
};

pub(super) async fn setup() -> (Rc<Kernel<HeadlessEnvironment>>, MapContext) {
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

#[tokio::test]
async fn each_consumer_applies_valid_results_after_an_invalid_payload() {
    use crate::{
        raster::RasterLayersDataComponent, sdf::SymbolLayersDataComponent,
        terrain::DemTileComponent,
    };
    let (kernel, mut context) = setup().await;
    let coords = WorldTileCoords::default();
    context
        .world
        .tiles
        .spawn_mut(coords)
        .expect("tile")
        .insert(VectorLayerBucketComponent::default())
        .insert(SymbolLayersDataComponent::default())
        .insert(RasterLayersDataComponent::default())
        .insert(DemTileComponent::Pending);
    for message in good_results(coords) {
        kernel
            .apc()
            .channel
            .0
            .send(Message::new(message.tag(), Box::new(17_u32)))
            .expect("invalid result");
        kernel.apc().channel.0.send(message).expect("valid result");
    }
    for mut system in consumers(&kernel) {
        assert!(matches!(
            system.run(&mut context),
            Err(SystemError::WorkerMessage(_))
        ));
    }
    assert!(
        context
            .world
            .tiles
            .query::<&VectorLayerBucketComponent>(coords)
            .expect("vectors")
            .done
    );
    assert_eq!(
        context
            .world
            .tiles
            .query::<&RasterLayersDataComponent>(coords)
            .expect("raster")
            .layers
            .len(),
        1
    );
    assert!(matches!(
        context.world.tiles.query::<&DemTileComponent>(coords),
        Some(DemTileComponent::Missing)
    ));
    assert_eq!(
        context
            .world
            .tiles
            .query::<&SymbolLayersDataComponent>(coords)
            .expect("symbols")
            .layers
            .len(),
        1
    );
}

fn good_results(coords: WorldTileCoords) -> Vec<Message> {
    use crate::{
        raster::{DefaultRasterTransferables, LayerRasterMissing, RasterTransferables},
        terrain::transferables::*,
        vector::tessellation::OverAlignedVertexBuffer,
    };
    vec![
        IntoMessage::into(DefaultTileTessellated::build_from(coords)),
        IntoMessage::into(
            <DefaultRasterTransferables as RasterTransferables>::LayerRasterMissing::build_from(
                coords,
            ),
        ),
        IntoMessage::into(DefaultLayerDemMissing::build_from(coords)),
        IntoMessage::into(DefaultSymbolLayerTessellated::build_from(
            coords,
            OverAlignedVertexBuffer::empty(),
            Vec::new(),
            None,
            Default::default(),
            "labels".into(),
        )),
    ]
}

fn consumers(kernel: &Rc<Kernel<HeadlessEnvironment>>) -> Vec<Box<dyn System>> {
    use crate::{
        raster::DefaultRasterTransferables, terrain::transferables::DefaultDemTransferables,
    };
    vec![
        Box::new(crate::vector::populate_world_system::PopulateWorldSystem::<
            HeadlessEnvironment,
            DefaultVectorTransferables,
        >::new(kernel)),
        Box::new(crate::raster::populate_world_system::PopulateWorldSystem::<
            HeadlessEnvironment,
            DefaultRasterTransferables,
        >::new(kernel)),
        Box::new(
            crate::terrain::populate_world_system::PopulateWorldSystem::<
                HeadlessEnvironment,
                DefaultDemTransferables,
            >::new(kernel),
        ),
        Box::new(crate::sdf::populate_world_system::PopulateWorldSystem::<
            HeadlessEnvironment,
            DefaultVectorTransferables,
        >::new(kernel)),
    ]
}
