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

#[test]
fn a_heatmap_point_is_met_within_its_radius_on_the_ground() {
    use crate::{
        coords::WorldTileCoords, io::geometry_index::ExactGeometry, style::heatmap::HeatmapPaint,
    };
    let tile = QueryTile {
        coords: WorldTileCoords::default(),
        wrap: 0,
        local: [100.0, 100.0, 100.0, 100.0],
        units_per_pixel: 2.0,
        zoom_level: 0,
        origin: [0.0, 0.0],
        world_per_unit: 0.5,
        footprint: crate::query::ground::Footprint::of(&[[100.0, 100.0]]),
    };
    let paint: HeatmapPaint =
        serde_json::from_value(serde_json::json!({"heatmap-radius": 10})).unwrap();
    let at = |x: f64| ExactGeometry::Point(Point::new(x, 100.0));
    let props = FeatureProperties::default();

    // Ten pixels are twenty tile units here.
    assert!(heatmap_hit(&at(119.0), &tile, (&paint, &props), 0.0));
    assert!(!heatmap_hit(&at(121.0), &tile, (&paint, &props), 0.0));
}
