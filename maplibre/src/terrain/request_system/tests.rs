#![allow(clippy::expect_used, clippy::panic)]

use super::{dem_ancestor_coords, dem_tile_coords};
use crate::coords::{WorldTileCoords, ZoomLevel};

fn tile(x: i32, y: i32, z: u8) -> WorldTileCoords {
    WorldTileCoords {
        x,
        y,
        z: ZoomLevel::new(z),
    }
}

#[test]
fn dem_tile_sits_one_zoom_level_above_the_view_tile() {
    assert_eq!(
        dem_tile_coords(tile(2201, 1453, 12), 0, 12),
        Some(tile(1100, 726, 11))
    );
    assert_eq!(dem_tile_coords(tile(0, 0, 0), 0, 12), Some(tile(0, 0, 0)));
}

#[test]
fn dem_tile_is_clamped_to_the_source_zoom_range() {
    assert_eq!(
        dem_tile_coords(tile(8804, 5812, 14), 0, 12),
        Some(tile(2201, 1453, 12))
    );
    assert_eq!(dem_tile_coords(tile(3, 2, 3), 5, 12), None);
}

#[test]
fn a_coarse_ancestor_accompanies_every_dem_tile_for_culling() {
    assert_eq!(
        dem_ancestor_coords(tile(1100, 726, 11), 0),
        Some(tile(17, 11, 5))
    );
    assert_eq!(dem_ancestor_coords(tile(17, 11, 5), 0), None);
    assert_eq!(dem_ancestor_coords(tile(3, 2, 3), 0), None);
    assert_eq!(
        dem_ancestor_coords(tile(1100, 726, 11), 8),
        Some(tile(137, 90, 8)),
        "the ancestor never drops below the source minimum zoom"
    );
}
