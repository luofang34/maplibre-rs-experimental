use rustc_hash::FxHashSet as HashSet;

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
    fade.insert("satellite", [tile(1, 1, 2)].into_iter().collect(), 0.5);

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
    fade.insert("early", HashSet::default(), 1.5);
    fade.insert("late", HashSet::default(), -0.5);

    assert_eq!(fade.departing(Some("early")).map(|d| d.opacity), Some(1.0));
    assert_eq!(fade.departing(Some("late")).map(|d| d.opacity), Some(0.0));
}

#[test]
fn live_timed_fade_tracks_departing_descendants_and_decays_to_completion() {
    use std::time::Duration;

    let mut fade = RasterCrossFade::default();
    let t0 = instant::Instant::now();
    let children: HashSet<_> = [tile(0, 0, 2), tile(1, 0, 2)].into_iter().collect();
    let parent: HashSet<_> = [tile(0, 0, 1)].into_iter().collect();

    // Frame 1: child tiles visible.
    fade.update_live_fades("sat", &children, 300, t0);
    assert!(!fade.has_active_timed_fade());

    // Frame 2: zoomed out to parent tile -> children become departing with opacity 1.0.
    fade.update_live_fades("sat", &parent, 300, t0);
    assert!(fade.has_active_timed_fade());
    assert!(fade.is_departing(Some("sat"), &tile(0, 0, 2)));
    assert_eq!(fade.departing(Some("sat")).map(|d| d.opacity), Some(1.0));

    // Halfway through 300ms fade -> opacity ~0.5, still animating.
    let active_mid = fade.advance(t0 + Duration::from_millis(150));
    assert!(active_mid);
    let mid_opacity = fade
        .departing(Some("sat"))
        .map(|d| d.opacity)
        .unwrap_or(0.0);
    assert!((mid_opacity - 0.5).abs() < 1e-3, "got {mid_opacity}");

    // After 300ms -> fade finishes and departing tiles are cleared.
    let active_end = fade.advance(t0 + Duration::from_millis(300));
    assert!(!active_end);
    assert!(fade.departing(Some("sat")).is_none());
}
