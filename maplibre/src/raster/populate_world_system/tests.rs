#![allow(clippy::expect_used, clippy::panic)]

use super::apply_raster_message;
use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    io::apc::IntoMessage,
    raster::{
        transferables::{
            DefaultLayerRasterMissing, DefaultRasterTransferables, LayerRasterMissing,
        },
        RasterLayerData, RasterLayersDataComponent,
    },
    tcs::world::World,
};

fn tile() -> WorldTileCoords {
    WorldTileCoords {
        x: 1,
        y: 2,
        z: ZoomLevel::new(3),
    }
}

#[test]
fn a_missing_raster_message_marks_the_layer_missing() {
    let mut world = World::default();
    world
        .tiles
        .spawn_mut(tile())
        .expect("valid tile coordinates")
        .insert(RasterLayersDataComponent::default());

    apply_raster_message::<DefaultRasterTransferables>(
        &mut world,
        IntoMessage::into(DefaultLayerRasterMissing::build_from(tile())),
    );

    let component = world
        .tiles
        .query::<&RasterLayersDataComponent>(tile())
        .expect("component present");
    assert!(
        matches!(component.layers.as_slice(), [RasterLayerData::Missing(layer)] if layer.coords == tile()),
        "the tile must carry the missing layer so it counts as done"
    );
}

#[test]
fn a_message_for_an_unknown_tile_is_dropped() {
    let mut world = World::default();

    apply_raster_message::<DefaultRasterTransferables>(
        &mut world,
        IntoMessage::into(DefaultLayerRasterMissing::build_from(tile())),
    );

    assert!(world
        .tiles
        .query::<&RasterLayersDataComponent>(tile())
        .is_none());
}
