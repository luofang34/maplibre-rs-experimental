use std::collections::HashSet;

use super::RasterCrossFade;
use crate::coords::{WorldTileCoords, ZoomLevel};

fn tile(x: i32, y: i32, z: u8) -> WorldTileCoords {
    WorldTileCoords {
        x,
        y,
        z: ZoomLevel::from(z),
    }
}

#[test]
fn a_departing_tile_belongs_to_its_own_source() {
    let mut fade = RasterCrossFade::default();
    fade.insert("satellite", HashSet::from([tile(1, 1, 2)]), 0.5);

    assert!(fade.is_departing(Some("satellite"), &tile(1, 1, 2)));
    assert!(!fade.is_departing(Some("satellite"), &tile(0, 0, 1)));
    assert!(!fade.is_departing(Some("other"), &tile(1, 1, 2)));
    assert!(!fade.is_departing(None, &tile(1, 1, 2)));
    assert_eq!(
        fade.departing(Some("satellite")).map(|d| d.opacity),
        Some(0.5)
    );
}

#[test]
fn the_opacity_left_stays_within_a_fade() {
    let mut fade = RasterCrossFade::default();
    fade.insert("early", HashSet::new(), 1.5);
    fade.insert("late", HashSet::new(), -0.5);

    assert_eq!(fade.departing(Some("early")).map(|d| d.opacity), Some(1.0));
    assert_eq!(fade.departing(Some("late")).map(|d| d.opacity), Some(0.0));
}
