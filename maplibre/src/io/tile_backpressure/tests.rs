#![allow(clippy::expect_used, clippy::panic)]

use super::{request_budget, tiles_in_flight, vector_request_budget, MAX_TILES_IN_FLIGHT};
use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    io::tile_retry::{
        completed, next_attempt, started, RequestDisposition, RequestKind, TileRequestOutcome,
    },
    render::memory_budget::{
        MemoryBudget, CRITICAL_MEMORY_BYTES, LOW_MEMORY_BYTES, TILES_IN_FLIGHT_WHEN_LOW,
    },
    tcs::world::World,
};

fn coords(x: i32, y: i32, z: u8) -> WorldTileCoords {
    WorldTileCoords {
        x,
        y,
        z: ZoomLevel::new(z),
    }
}

/// Admits a request as a request system does, returning its attempt.
fn request(world: &mut World, coords: WorldTileCoords, kind: RequestKind) -> u64 {
    let attempt = next_attempt(world);
    started(world, coords, kind, attempt, None);
    attempt
}

fn complete(world: &mut World, coords: WorldTileCoords, kind: RequestKind, attempt: u64) {
    completed(
        world,
        TileRequestOutcome {
            coords,
            kind,
            attempt: Some(attempt),
            disposition: RequestDisposition::Complete,
        },
    );
}

#[test]
fn the_budget_counts_every_unfinished_request_and_saturates_at_zero() {
    let mut world = World::default();
    assert_eq!(request_budget(&world), MAX_TILES_IN_FLIGHT);

    let attempts: Vec<_> = (0..MAX_TILES_IN_FLIGHT as i32 + 5)
        .map(|x| request(&mut world, coords(x, 0, 6), RequestKind::Vector))
        .collect();
    request(&mut world, coords(0, 1, 6), RequestKind::Dem);
    assert_eq!(tiles_in_flight(&world), MAX_TILES_IN_FLIGHT + 6);
    assert_eq!(request_budget(&world), 0);

    for (x, attempt) in attempts.iter().enumerate().take(10) {
        complete(
            &mut world,
            coords(x as i32, 0, 6),
            RequestKind::Vector,
            *attempt,
        );
    }
    assert_eq!(tiles_in_flight(&world), MAX_TILES_IN_FLIGHT - 4);
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
    let attempts: Vec<_> = (0..vectors as i32)
        .map(|x| request(&mut world, coords(x, 0, 6), RequestKind::Vector))
        .collect();
    let terrain = request_budget(&world);
    assert!(terrain > 0);
    for x in 0..terrain as i32 {
        request(&mut world, coords(x, 1, 6), RequestKind::Dem);
    }
    assert_eq!(vector_request_budget(&world, true), 0);
    complete(
        &mut world,
        coords(0, 0, 6),
        RequestKind::Vector,
        attempts[0],
    );
    assert_eq!(vector_request_budget(&world, true), 1);
    world.resources.insert(MemoryBudget {
        available_bytes: Some(CRITICAL_MEMORY_BYTES - 1),
    });
    assert_eq!(vector_request_budget(&world, true), 0);
}
