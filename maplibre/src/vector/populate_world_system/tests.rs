#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use crate::{
    coords::WorldTileCoords,
    io::tile_backpressure::{request_budget, MAX_TILES_IN_FLIGHT},
    sdf::SymbolLayersDataComponent,
    tcs::world::World,
};
#[test]
fn visible_base_geometry_keeps_its_loading_slot_until_symbol_assets_finish() {
    let mut world = World::default();
    let coords = WorldTileCoords::default();
    world
        .tiles
        .spawn_mut(coords)
        .expect("tile")
        .insert(VectorLayerBucketComponent::default())
        .insert(SymbolLayersDataComponent::default());
    finish_tile(&mut world, &DefaultTileTessellated::build_partial(coords));
    assert!(
        world
            .tiles
            .query::<&VectorLayerBucketComponent>(coords)
            .expect("base")
            .done
    );
    assert_eq!(request_budget(&world), MAX_TILES_IN_FLIGHT - 1);
    finish_tile(&mut world, &DefaultTileTessellated::build_from(coords));
    assert_eq!(request_budget(&world), MAX_TILES_IN_FLIGHT);
}
