#![allow(clippy::expect_used, clippy::panic)]

use cgmath::Vector4;

use super::drape_transform;
use crate::coords::{WorldTileCoords, ZoomLevel, EXTENT};

fn tile(x: i32, y: i32, z: u8) -> WorldTileCoords {
    WorldTileCoords {
        x,
        y,
        z: ZoomLevel::new(z),
    }
}

fn ndc(target: WorldTileCoords, source: WorldTileCoords, x: f64, y: f64) -> (f64, f64) {
    let clip =
        drape_transform(target, source).expect("related tiles") * Vector4::new(x, y, 0.0, 1.0);
    (clip.x / clip.w, clip.y / clip.w)
}

#[test]
fn the_tile_itself_fills_the_texture_with_row_zero_on_top() {
    let t = tile(5, 3, 4);
    assert_eq!(ndc(t, t, 0.0, 0.0), (-1.0, 1.0));
    assert_eq!(ndc(t, t, EXTENT, EXTENT), (1.0, -1.0));
    assert_eq!(ndc(t, t, EXTENT / 2.0, EXTENT / 2.0), (0.0, 0.0));
}

#[test]
fn a_parent_maps_its_quadrant_over_the_whole_texture() {
    let target = tile(3, 2, 2);
    let parent = tile(1, 1, 1);
    // Target (3, 2) is the top-right child of (1, 1): its square starts halfway across the
    // parent's top row.
    assert_eq!(ndc(target, parent, EXTENT / 2.0, 0.0), (-1.0, 1.0));
    assert_eq!(ndc(target, parent, EXTENT, EXTENT / 2.0), (1.0, -1.0));
}

#[test]
fn a_child_shrinks_into_its_quadrant() {
    let target = tile(1, 1, 1);
    let child = tile(3, 2, 2);
    assert_eq!(ndc(target, child, 0.0, 0.0), (0.0, 1.0));
    assert_eq!(ndc(target, child, EXTENT, EXTENT), (1.0, 0.0));
}

#[test]
fn unrelated_tiles_have_no_transform() {
    assert!(drape_transform(tile(0, 0, 2), tile(1, 1, 1)).is_none());
    assert!(drape_transform(tile(0, 0, 1), tile(3, 3, 2)).is_none());
    assert!(drape_transform(tile(0, 0, 1), tile(1, 0, 1)).is_none());
}

#[test]
fn layer_depth_stays_inside_the_clip_range() {
    let t = tile(0, 0, 0);
    let clip = drape_transform(t, t).expect("same tile") * Vector4::new(0.0, 0.0, 0.0, 1.0);
    // Shaders overwrite z with the layer index; it must divide to well below one.
    assert!(clip.w > 1000.0);
}
