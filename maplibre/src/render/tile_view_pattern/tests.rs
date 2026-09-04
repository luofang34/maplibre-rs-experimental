#![allow(clippy::expect_used, clippy::panic)]

use std::collections::HashSet;

use super::HasTile;
use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    tcs::world::World,
};

struct Loaded(HashSet<WorldTileCoords>);

impl HasTile for Loaded {
    fn has_tile(&self, coords: WorldTileCoords, _world: &World) -> bool {
        self.0.contains(&coords)
    }
}

fn tile(x: i32, y: i32, z: u8) -> WorldTileCoords {
    WorldTileCoords {
        x,
        y,
        z: ZoomLevel::new(z),
    }
}

#[test]
fn complete_children_cover_the_tile_across_depths() {
    let world = World::default();
    let parent = tile(1, 1, 2);
    let [a, b, c, d] = parent.get_children();
    let mut loaded: HashSet<WorldTileCoords> = [a, b, c].into();
    loaded.extend(d.get_children());
    let container = Loaded(loaded);

    let children = container
        .get_complete_children(parent, &world, 2)
        .expect("children at two depths cover the parent");

    assert_eq!(children.len(), 7);
    assert!(container.get_complete_children(parent, &world, 1).is_none());
}

#[test]
fn incomplete_children_do_not_stand_in_for_the_tile() {
    let world = World::default();
    let parent = tile(1, 1, 2);
    let [a, b, _, _] = parent.get_children();
    let container = Loaded([a, b].into());

    assert!(container.get_complete_children(parent, &world, 4).is_none());
    assert_eq!(
        container.get_available_children(parent, &world, 4),
        Some(vec![a, b])
    );
}
