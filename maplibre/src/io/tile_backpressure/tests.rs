#![allow(clippy::expect_used, clippy::panic)]

use super::{request_budget, tiles_in_flight, vector_request_budget, MAX_TILES_IN_FLIGHT};
use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    render::memory_budget::{
        MemoryBudget, CRITICAL_MEMORY_BYTES, LOW_MEMORY_BYTES, TILES_IN_FLIGHT_WHEN_LOW,
    },
    tcs::world::World,
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
    let mut world = World::default();
    assert_eq!(request_budget(&world), MAX_TILES_IN_FLIGHT);
    let tiles = &mut world.tiles;

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
    assert_eq!(tiles_in_flight(tiles), MAX_TILES_IN_FLIGHT + 6);
    assert_eq!(request_budget(&world), 0);
    let tiles = &mut world.tiles;

    for x in 0..10 {
        tiles
            .query_mut::<&mut VectorLayerBucketComponent>(coords(x, 0, 6))
            .expect("a requested tile")
            .done = true;
    }
    assert_eq!(tiles_in_flight(tiles), MAX_TILES_IN_FLIGHT - 4);
    assert_eq!(request_budget(&world), 4);
}

#[test]
fn few_tiles_are_requested_while_memory_is_low_and_none_while_it_is_critical() {
    let mut world = World::default();
    world.resources.insert(MemoryBudget {
        available_bytes: Some(LOW_MEMORY_BYTES - 1),
    });
    assert_eq!(request_budget(&world), TILES_IN_FLIGHT_WHEN_LOW);
    world.resources.insert(MemoryBudget {
        available_bytes: Some(CRITICAL_MEMORY_BYTES - 1),
    });
    assert_eq!(request_budget(&world), 0);
    world.resources.insert(MemoryBudget {
        available_bytes: Some(LOW_MEMORY_BYTES),
    });
    assert_eq!(request_budget(&world), MAX_TILES_IN_FLIGHT);
}

#[test]
fn terrain_and_vectors_both_make_progress_under_low_memory() {
    let mut world = World::default();
    world.resources.insert(MemoryBudget {
        available_bytes: Some(LOW_MEMORY_BYTES - 1),
    });
    let vectors = vector_request_budget(&world, true);
    assert!(vectors > 0);
    assert!(vectors < request_budget(&world));
    for x in 0..vectors as i32 {
        world
            .tiles
            .spawn_mut(coords(x, 0, 6))
            .expect("vector tile")
            .insert(VectorLayerBucketComponent::default());
    }
    let terrain = request_budget(&world);
    assert!(terrain > 0);
    for x in 0..terrain as i32 {
        world
            .tiles
            .spawn_mut(coords(x, 1, 6))
            .expect("terrain tile")
            .insert(DemTileComponent::Pending);
    }
    assert_eq!(vector_request_budget(&world, true), 0);
    world
        .tiles
        .query_mut::<&mut VectorLayerBucketComponent>(coords(0, 0, 6))
        .expect("requested vector tile")
        .done = true;
    assert_eq!(vector_request_budget(&world, true), 1);
    world.resources.insert(MemoryBudget {
        available_bytes: Some(CRITICAL_MEMORY_BYTES - 1),
    });
    assert_eq!(vector_request_budget(&world, true), 0);
}
