#![allow(clippy::panic)]

use super::pan_center_to_anchor;
use crate::coords::LatLon;

fn assert_close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
}

#[test]
fn identical_anchor_leaves_center_and_zoom_unchanged() {
    let center = LatLon::new(15.0, 10.0);
    let update = pan_center_to_anchor(center, 20.0, center, center);

    assert_close(update.center.latitude, center.latitude);
    assert_close(update.center.longitude, center.longitude);
    assert_close(update.zoom_adjustment, 0.0);
}

#[test]
fn pan_rotation_stays_finite_near_pole_and_antimeridian() {
    let update = pan_center_to_anchor(
        LatLon::new(80.0, 179.0),
        0.0,
        LatLon::new(85.0, -179.0),
        LatLon::new(84.0, 175.0),
    );

    assert!(update.center.latitude.is_finite());
    assert!(update.center.longitude.is_finite());
    assert!(update.zoom_adjustment.is_finite());
    assert!((-90.0..=90.0).contains(&update.center.latitude));
    assert!((-180.0..180.0).contains(&update.center.longitude));
}

#[test]
fn horizontal_drag_rotates_center_in_opposite_direction() {
    let update = pan_center_to_anchor(
        LatLon::new(0.0, 0.0),
        0.0,
        LatLon::new(0.0, -10.0),
        LatLon::new(0.0, 0.0),
    );

    assert_close(update.center.latitude, 0.0);
    assert_close(update.center.longitude, -10.0);
}
