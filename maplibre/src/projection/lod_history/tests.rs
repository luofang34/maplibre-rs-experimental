use super::*;
use crate::{
    coords::ZoomLevel,
    projection::globe::covering_tiles::{lod::LodContext, ZoomRounding},
};
use cgmath::Point2;

#[test]
fn jitter_keeps_a_leaf_until_split_and_merge_thresholds_are_crossed() {
    let tile = TileCoords::from((0, 0, ZoomLevel::new(8)));
    let parent = WorldTileCoords {
        x: 0,
        y: 0,
        z: tile.z,
    };
    let leaf = LodHistory::new(&[parent]);
    let refined = LodHistory::new(&[WorldTileCoords {
        x: 0,
        y: 0,
        z: ZoomLevel::new(9),
    }]);
    let select = |zoom: f64, history: &LodHistory| {
        LodContext::from_eye(Point2::new(0.0, 0.0), 1.0, 512.0 * 2_f64.powf(zoom))
            .stable_zoom_for_tile(tile, ZoomRounding::Floor, Some(history))
    };
    for zoom in [8.99, 9.01, 8.95, 9.05] {
        assert_eq!(select(zoom, &leaf), ZoomLevel::new(8));
        assert_eq!(select(zoom, &refined), ZoomLevel::new(9));
    }
    assert_eq!(select(9.16, &leaf), ZoomLevel::new(9));
    assert_eq!(select(8.84, &refined), ZoomLevel::new(8));
    // Fast motion changes the required level immediately; no frame-count delay applies.
    assert_eq!(select(12.0, &leaf), ZoomLevel::new(11));
    assert_eq!(leaf.bias(TileCoords::from((4, 4, ZoomLevel::new(8)))), 0.0);
}
