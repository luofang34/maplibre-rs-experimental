#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use crate::{
    coords::WorldTileCoords,
    io::tile_backpressure::{request_budget, MAX_TILES_IN_FLIGHT},
    sdf::SymbolLayersDataComponent,
    tcs::world::World,
};
#[test]
fn visible_base_geometry_keeps_its_loading_slot_until_its_request_lands() {
    use crate::io::tile_retry::{self, RequestDisposition, RequestKind, TileRequestOutcome};
    let mut world = World::default();
    let coords = WorldTileCoords::default();
    world
        .tiles
        .spawn_mut(coords)
        .expect("tile")
        .insert(VectorLayerBucketComponent::default())
        .insert(SymbolLayersDataComponent::default());
    let attempt = tile_retry::next_attempt(&mut world);
    tile_retry::started(&mut world, coords, RequestKind::Vector, attempt, None);
    finish_tile(&mut world, &DefaultTileTessellated::build_partial(coords));
    assert!(
        world
            .tiles
            .query::<&VectorLayerBucketComponent>(coords)
            .expect("base")
            .done
    );
    assert_eq!(request_budget(&world), MAX_TILES_IN_FLIGHT - 1);
    // The worker completes the labels before it reports the request's outcome.
    finish_tile(&mut world, &DefaultTileTessellated::build_from(coords));
    assert_eq!(request_budget(&world), MAX_TILES_IN_FLIGHT - 1);
    tile_retry::completed(
        &mut world,
        TileRequestOutcome {
            coords,
            kind: RequestKind::Vector,
            attempt: Some(attempt),
            disposition: RequestDisposition::Complete,
        },
    );
    assert_eq!(request_budget(&world), MAX_TILES_IN_FLIGHT);
}
