use cgmath::Deg;

use super::*;
use crate::{
    coords::{WorldCoords, Zoom},
    window::PhysicalSize,
};

fn view(pitch: f64) -> ViewState {
    let centre = 256.0 * 2.0_f64.powf(10.0);
    ViewState::new(
        PhysicalSize::new(800, 600).unwrap(),
        WorldCoords::at_ground(centre, centre),
        Zoom::new(10.0),
        Deg(pitch),
        Deg(36.87),
    )
}

#[test]
fn the_point_under_the_screen_centre_is_as_far_as_the_camera_is_from_the_centre() {
    let view = view(45.0);
    let centre = 256.0 * 2.0_f64.powf(10.0);
    let (window, w) = project(&view, [centre, centre]).unwrap();

    assert!((window[0] - 400.0).abs() < 1e-6 && (window[1] - 300.0).abs() < 1e-6);
    assert!(
        (w - view.camera_to_center_distance()).abs() < 1e-6 * w,
        "clip w {w} is the distance {} GL JS scales circles by",
        view.camera_to_center_distance()
    );
}

#[test]
fn a_nearer_point_of_a_pitched_view_is_closer_to_the_camera() {
    let view = view(60.0);
    let centre = 256.0 * 2.0_f64.powf(10.0);
    let (_, far) = project(&view, [centre, centre - 100.0]).unwrap();
    let (_, near) = project(&view, [centre, centre + 100.0]).unwrap();

    assert!(near < view.camera_to_center_distance() && far > view.camera_to_center_distance());
}

#[test]
fn size_is_radius_and_stroke_with_the_default_radius() {
    let paint: CirclePaint =
        serde_json::from_value(serde_json::json!({"circle-stroke-width": 2})).unwrap();

    assert_eq!(size_pixels(&paint, &FeatureProperties::default(), 0.0), 7.0);
}
