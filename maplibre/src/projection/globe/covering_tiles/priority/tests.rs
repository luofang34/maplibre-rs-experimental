#![allow(clippy::expect_used, clippy::panic)]
use super::{add_padding, sort_by_center};
use crate::coords::{LatLon, WorldTileCoords, ZoomLevel};

#[test]
fn mixed_zoom_tiles_are_sorted_in_one_coordinate_space() {
    let nearby = WorldTileCoords {
        x: 8,
        y: 8,
        z: ZoomLevel::new(4),
    };
    let distant = WorldTileCoords {
        x: 128,
        y: 64,
        z: ZoomLevel::new(8),
    };
    let mut tiles = vec![distant, nearby];
    sort_by_center(&mut tiles, LatLon::new(0.0, 0.0), ZoomLevel::new(8));
    assert_eq!(tiles[0], nearby, "the nearby lower-zoom tile wins priority");
}

#[test]
fn padding_does_not_replace_visible_tiles_with_neighbors() {
    let visible: Vec<_> = (0..20)
        .map(|x| WorldTileCoords {
            x,
            y: 20,
            z: ZoomLevel::new(8),
        })
        .collect();
    let padded = add_padding(visible.clone(), 1, visible.len());
    assert_eq!(padded, visible);
}
