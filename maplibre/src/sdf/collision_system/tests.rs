#![allow(clippy::expect_used, clippy::panic)]

use super::canonical_tile;
use crate::coords::{WorldTileCoords, ZoomLevel};

#[test]
fn canonical_symbol_tile_wraps_antimeridian_copies() {
    let zoom = ZoomLevel::new(2);

    assert_eq!(
        canonical_tile(WorldTileCoords {
            x: -1,
            y: 1,
            z: zoom
        })
        .map(|tile| tile.x),
        Some(3)
    );
    assert_eq!(
        canonical_tile(WorldTileCoords {
            x: 4,
            y: 1,
            z: zoom
        })
        .map(|tile| tile.x),
        Some(0)
    );
}

#[test]
fn canonical_symbol_tile_rejects_polar_overflow() {
    let zoom = ZoomLevel::new(2);

    assert!(canonical_tile(WorldTileCoords {
        x: 0,
        y: -1,
        z: zoom
    })
    .is_none());
    assert!(canonical_tile(WorldTileCoords {
        x: 0,
        y: 4,
        z: zoom
    })
    .is_none());
}

#[test]
fn the_hosts_obstacles_are_in_the_grid_before_any_label() {
    let mut obstacles = crate::sdf::obstacles::LabelObstacles::default();
    obstacles.replace(vec![[100.0, 100.0, 140.0, 110.0]]);
    let grid = super::obstructed_grid(800.0, 600.0, Some(&obstacles));
    assert!(
        grid.overlaps([130.0, 105.0, 200.0, 120.0]),
        "a label over the obstacle"
    );
    assert!(
        grid.overlaps_non_cooperative([130.0, 105.0, 200.0, 120.0]),
        "even one that lets cooperative labels overlap it"
    );
    assert!(
        !grid.overlaps([150.0, 105.0, 200.0, 120.0]),
        "a label beside it"
    );
    assert!(!super::obstructed_grid(800.0, 600.0, None).overlaps([130.0, 105.0, 200.0, 120.0]));
}
