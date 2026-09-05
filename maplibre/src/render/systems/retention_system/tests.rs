#![allow(clippy::expect_used, clippy::panic)]

use std::collections::HashSet;

use super::{evict_stale_tiles, MIN_CACHE_TILES};
use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    tcs::world::World,
    terrain::DemTileComponent,
    vector::VectorLayerBucketComponent,
};

fn tile(index: i32) -> WorldTileCoords {
    WorldTileCoords {
        x: index % 32,
        y: index / 32,
        z: ZoomLevel::new(5),
    }
}

fn spawn_vector(world: &mut World, coords: WorldTileCoords, done: bool) {
    world
        .tiles
        .spawn_mut(coords)
        .expect("valid coordinates")
        .insert(VectorLayerBucketComponent {
            done,
            layers: Vec::new(),
        });
}

#[test]
fn tiles_beyond_the_budget_are_evicted_but_loading_and_in_use_tiles_stay() {
    let mut world = World::default();
    let extra = 10;
    for index in 0..(MIN_CACHE_TILES + extra) as i32 {
        spawn_vector(&mut world, tile(index), true);
    }
    let loading = tile(500);
    spawn_vector(&mut world, loading, false);
    let in_use = tile(600);
    spawn_vector(&mut world, in_use, true);
    world
        .tiles
        .spawn_mut(tile(700))
        .expect("valid coordinates")
        .insert(DemTileComponent::Pending);

    let evicted = evict_stale_tiles(&mut world, &HashSet::from([in_use]));

    assert_eq!(evicted.len(), extra);
    assert!(
        world.tiles.exists(loading),
        "a tile still loading is never evicted"
    );
    assert!(world.tiles.exists(in_use));
    assert!(
        world.tiles.exists(tile(700)),
        "a pending DEM tile is never evicted"
    );
    assert_eq!(
        world.tiles.tiles.len(),
        MIN_CACHE_TILES + 3,
        "the cache keeps exactly the budget plus the protected tiles"
    );
    for coords in evicted {
        assert!(!world.tiles.exists(coords));
    }
}

#[test]
fn recently_used_tiles_outlive_never_used_ones() {
    let mut world = World::default();
    let recent = tile(0);
    spawn_vector(&mut world, recent, true);
    assert!(evict_stale_tiles(&mut world, &HashSet::from([recent])).is_empty());

    for index in 1..=(MIN_CACHE_TILES + 3) as i32 {
        spawn_vector(&mut world, tile(index), true);
    }
    let evicted = evict_stale_tiles(&mut world, &HashSet::new());

    assert_eq!(evicted.len(), 4);
    assert!(!evicted.contains(&recent));
    assert!(world.tiles.exists(recent));
}

#[test]
fn nothing_is_evicted_within_the_budget() {
    let mut world = World::default();
    for index in 0..8 {
        spawn_vector(&mut world, tile(index), true);
    }
    assert!(evict_stale_tiles(&mut world, &HashSet::new()).is_empty());
    assert_eq!(world.tiles.tiles.len(), 8);
}
