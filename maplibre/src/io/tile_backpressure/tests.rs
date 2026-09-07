#![allow(clippy::expect_used, clippy::panic)]

use super::{request_budget, tiles_in_flight, MAX_TILES_IN_FLIGHT};
use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    tcs::tiles::Tiles,
    terrain::DemTileComponent,
    vector::VectorLayerBucketComponent,
};

fn coords(x: i32, y: i32, z: u8) -> WorldTileCoords {
    WorldTileCoords {
        x,
        y,
        z: ZoomLevel::new(z),
    }
}

#[test]
fn the_budget_counts_every_unfinished_request_and_saturates_at_zero() {
    let mut tiles = Tiles::default();
    assert_eq!(request_budget(&tiles), MAX_TILES_IN_FLIGHT);

    for x in 0..MAX_TILES_IN_FLIGHT as i32 + 5 {
        tiles
            .spawn_mut(coords(x, 0, 6))
            .expect("a tile")
            .insert(VectorLayerBucketComponent::default());
    }
    tiles
        .spawn_mut(coords(0, 1, 6))
        .expect("a tile")
        .insert(DemTileComponent::Pending);
    assert_eq!(tiles_in_flight(&tiles), MAX_TILES_IN_FLIGHT + 6);
    assert_eq!(request_budget(&tiles), 0);

    for x in 0..10 {
        tiles
            .query_mut::<&mut VectorLayerBucketComponent>(coords(x, 0, 6))
            .expect("a requested tile")
            .done = true;
    }
    assert_eq!(tiles_in_flight(&tiles), MAX_TILES_IN_FLIGHT - 4);
    assert_eq!(request_budget(&tiles), 4);
}
