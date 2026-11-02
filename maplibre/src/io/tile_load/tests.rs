#![allow(clippy::expect_used, clippy::panic)]

use std::collections::HashSet;

use super::{is_loading, loading_count, LoadState, TileLoad};
use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    io::tile_retry::{
        cancel_unwanted, completed, next_attempt, started, want, RequestDisposition, RequestKind,
        TileRequestOutcome,
    },
    tcs::world::World,
};

fn coords(x: i32) -> WorldTileCoords {
    WorldTileCoords {
        x,
        y: 0,
        z: ZoomLevel::new(4),
    }
}

/// Counts the loading tiles by visiting every tile.
fn scanned(world: &World) -> usize {
    world
        .tiles
        .tiles
        .values()
        .filter(|tile| is_loading(&world.tiles, tile.coords))
        .count()
}

fn issue(world: &mut World, x: i32, kind: RequestKind) -> u64 {
    let attempt = next_attempt(world);
    started(world, coords(x), kind, attempt, None);
    attempt
}

fn land(
    world: &mut World,
    x: i32,
    kind: RequestKind,
    attempt: u64,
    disposition: RequestDisposition,
) {
    completed(
        world,
        TileRequestOutcome {
            coords: coords(x),
            kind,
            attempt: Some(attempt),
            disposition,
        },
    );
}

fn state(world: &World, x: i32, kind: RequestKind) -> LoadState {
    world
        .tiles
        .query::<&TileLoad>(coords(x))
        .expect("requested tile")
        .state(kind)
}

#[test]
fn the_loading_count_matches_a_scan_across_issue_completion_and_eviction() {
    let mut world = World::default();
    let vector: Vec<_> = (0..5)
        .map(|x| issue(&mut world, x, RequestKind::Vector))
        .collect();
    let dem = issue(&mut world, 0, RequestKind::Dem);
    assert_eq!(
        loading_count(&world),
        5,
        "one tile with two requests counts once"
    );
    assert_eq!(loading_count(&world), scanned(&world));

    land(
        &mut world,
        0,
        RequestKind::Vector,
        vector[0],
        RequestDisposition::Complete,
    );
    assert_eq!(loading_count(&world), 5, "its DEM is still loading");
    land(
        &mut world,
        0,
        RequestKind::Dem,
        dem,
        RequestDisposition::Complete,
    );
    land(
        &mut world,
        1,
        RequestKind::Vector,
        vector[1],
        RequestDisposition::Retry,
    );
    assert_eq!(loading_count(&world), 3);
    assert_eq!(loading_count(&world), scanned(&world));

    assert!(world.tiles.remove(coords(2)), "evicted while loading");
    assert_eq!(loading_count(&world), 2);
    issue(&mut world, 2, RequestKind::Raster);
    assert_eq!(
        loading_count(&world),
        3,
        "the coordinates are requested anew"
    );
    assert_eq!(loading_count(&world), scanned(&world));

    want(&mut world, RequestKind::Vector, &HashSet::from([coords(4)]));
    cancel_unwanted(&mut world, &HashSet::new());
    assert_eq!(loading_count(&world), 2, "tile 3 was given up");
    assert_eq!(loading_count(&world), scanned(&world));
}

#[test]
fn a_request_moves_a_tile_through_the_gl_js_states() {
    let mut world = World::default();
    let first = issue(&mut world, 0, RequestKind::Raster);
    assert_eq!(state(&world, 0, RequestKind::Raster), LoadState::Loading);
    land(
        &mut world,
        0,
        RequestKind::Raster,
        first,
        RequestDisposition::Retry,
    );
    assert_eq!(state(&world, 0, RequestKind::Raster), LoadState::Errored);
    let retry = issue(&mut world, 0, RequestKind::Raster);
    assert_eq!(state(&world, 0, RequestKind::Raster), LoadState::Loading);
    land(
        &mut world,
        0,
        RequestKind::Raster,
        retry,
        RequestDisposition::Complete,
    );
    assert_eq!(state(&world, 0, RequestKind::Raster), LoadState::Loaded);
    let refresh = issue(&mut world, 0, RequestKind::Raster);
    assert_eq!(state(&world, 0, RequestKind::Raster), LoadState::Reloading);
    land(
        &mut world,
        0,
        RequestKind::Raster,
        first,
        RequestDisposition::Complete,
    );
    assert_eq!(
        state(&world, 0, RequestKind::Raster),
        LoadState::Reloading,
        "an older attempt's outcome does not settle the newer request"
    );
    want(&mut world, RequestKind::Raster, &HashSet::new());
    cancel_unwanted(&mut world, &HashSet::new());
    assert_eq!(
        state(&world, 0, RequestKind::Raster),
        LoadState::Loaded,
        "a given-up reload keeps the loaded result, {refresh}"
    );
}
