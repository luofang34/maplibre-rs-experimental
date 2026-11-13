#![allow(clippy::expect_used, clippy::panic)]

use cgmath::{Deg, Point2, Vector2, Vector4};

use super::*;
use crate::{
    coords::{LatLon, WorldCoords, Zoom, TILE_SIZE},
    render::view_state::{CameraOptions, FlightCurve, FlyOptions},
    window::PhysicalSize,
};

fn test_view(width: u32, height: u32, center: LatLon, zoom: f64) -> ViewState {
    let z = Zoom::new(zoom);
    ViewState::new(
        PhysicalSize::new(width, height).expect("valid viewport"),
        WorldCoords::from_lat_lon(center, z),
        z,
        Deg(0.0),
        Deg(36.869_897_645_844_02),
    )
}

fn project_lat_lon_to_window(view: &ViewState, location: LatLon) -> Point2<f64> {
    let world_size = TILE_SIZE * 2.0_f64.powf(view.zoom().value());
    let m = mercator_from_lat_lon(location);
    let world = Vector4::new(m.x * world_size, m.y * world_size, 0.0, 1.0);
    let clip = view.view_projection().project(world);
    let win = view.clip_to_window(&clip);
    Point2::new(win.x, win.y)
}

#[test]
fn min_and_max_zoom_clamp_set_zoom_jump_and_flight() {
    let mut view = test_view(800, 600, LatLon::new(47.3769, 8.5417), 10.0);
    view.set_min_zoom(Some(4.0));
    view.set_max_zoom(Some(15.0));
    assert_eq!(view.min_zoom(), Some(4.0));
    assert_eq!(view.max_zoom(), Some(15.0));

    view.set_zoom(Zoom::new(20.0));
    assert!((view.zoom().value() - 15.0).abs() < 1e-12);
    assert!((view.center_lat_lon().latitude - 47.3769).abs() < 1e-5);
    assert!((view.center_lat_lon().longitude - 8.5417).abs() < 1e-5);

    view.zoom_to(Zoom::new(1.5));
    assert!((view.zoom().value() - 4.0).abs() < 1e-12);

    view.jump_to(CameraOptions {
        zoom: Some(19.5),
        ..Default::default()
    })
    .expect("jump succeeds");
    assert!((view.zoom().value() - 15.0).abs() < 1e-12);

    // Tightening max_zoom below current zoom immediately clamps the view while keeping center.
    view.set_max_zoom(Some(12.0));
    assert!((view.zoom().value() - 12.0).abs() < 1e-12);
    assert!((view.center_lat_lon().latitude - 47.3769).abs() < 1e-5);

    // Flight path target zoom also clamps to [min_zoom, max_zoom].
    let flight = view.flight_path(
        CameraOptions {
            center: Some(LatLon::new(46.2, 6.15)),
            zoom: Some(22.0),
            ..Default::default()
        },
        FlightCurve::Fly(FlyOptions::default()),
    );
    for step in 1..=20 {
        let opts = flight.at(std::time::Duration::from_millis(step * 500), 0.0, 0.0);
        view.jump_to(opts).expect("flight step jump");
        assert!(view.zoom().value() <= 12.0 + 1e-9);
        assert!(view.zoom().value() >= 4.0 - 1e-9);
    }
    assert!((view.zoom().value() - 12.0).abs() < 1e-9);
}

#[test]
fn max_bounds_clamps_pan_jump_update_camera_and_flight() {
    let zurich_box = LatLonBounds::new(8.45, 47.32, 8.62, 47.44);
    let mut view = test_view(800, 600, LatLon::new(48.8566, 2.3522), 11.0);

    // Setting max_bounds immediately pulls an out-of-bounds center into the box.
    view.set_max_bounds(Some(zurich_box));
    let clamped = view.center_lat_lon();
    assert!(
        zurich_box.contains(clamped)
            || (clamped.latitude - 47.44).abs() < 1e-6
            || (clamped.longitude - 8.45).abs() < 1e-6
    );
    assert!((clamped.longitude - 8.45).abs() < 1e-6);
    assert!((clamped.latitude - 47.44).abs() < 1e-6);

    // Panning via update_camera / move_relative cannot escape max_bounds.
    view.update_camera(|cam| cam.move_relative(Vector2::new(-50_000.0, -50_000.0)));
    let after_pan = view.center_lat_lon();
    assert!((after_pan.longitude - 8.45).abs() < 1e-6);
    assert!((after_pan.latitude - 47.44).abs() < 1e-6);

    // Jump to southeast outside the box clamps to (south=47.32, east=8.62).
    view.jump_to(CameraOptions {
        center: Some(LatLon::new(45.0, 12.0)),
        ..Default::default()
    })
    .expect("jump succeeds");
    let se = view.center_lat_lon();
    assert!((se.latitude - 47.32).abs() < 1e-6);
    assert!((se.longitude - 8.62).abs() < 1e-6);
}

#[test]
fn camera_for_bounds_and_fit_bounds_respect_asymmetric_padding() {
    let mut view = test_view(800, 600, LatLon::new(0.0, 0.0), 2.0);
    let bounds = LatLonBounds::new(-10.0, 40.0, 10.0, 52.0);
    let padding = EdgePadding::new(40.0, 120.0, 80.0, 60.0);

    let fit = view.fit_bounds(bounds, padding).expect("fit_bounds");
    assert!((view.zoom().value() - fit.zoom).abs() < 1e-12);

    let corners = [
        LatLon::new(bounds.north, bounds.west),
        LatLon::new(bounds.north, bounds.east),
        LatLon::new(bounds.south, bounds.east),
        LatLon::new(bounds.south, bounds.west),
    ];
    let projected: Vec<Point2<f64>> = corners
        .into_iter()
        .map(|c| project_lat_lon_to_window(&view, c))
        .collect();

    let min_x = projected.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
    let max_x = projected
        .iter()
        .map(|p| p.x)
        .fold(f64::NEG_INFINITY, f64::max);
    let min_y = projected.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
    let max_y = projected
        .iter()
        .map(|p| p.y)
        .fold(f64::NEG_INFINITY, f64::max);

    // Every corner must sit inside the padded viewport [60..=680, 40..=520].
    assert!(min_x >= padding.left - 1e-6, "min_x={min_x}");
    assert!(max_x <= 800.0 - padding.right + 1e-6, "max_x={max_x}");
    assert!(min_y >= padding.top - 1e-6, "min_y={min_y}");
    assert!(max_y <= 600.0 - padding.bottom + 1e-6, "max_y={max_y}");

    // The bounding box center on screen matches the padded viewport center (370, 280).
    let cx = (min_x + max_x) * 0.5;
    let cy = (min_y + max_y) * 0.5;
    assert!((cx - 370.0).abs() < 1e-5, "cx={cx}");
    assert!((cy - 280.0).abs() < 1e-5, "cy={cy}");

    // And at least one axis touches the padded boundary tightly.
    let touches_x =
        (min_x - padding.left).abs() < 1e-4 && (max_x - (800.0 - padding.right)).abs() < 1e-4;
    let touches_y =
        (min_y - padding.top).abs() < 1e-4 && (max_y - (600.0 - padding.bottom)).abs() < 1e-4;
    assert!(touches_x || touches_y, "fit must be tight on limiting axis");
}

#[test]
fn camera_for_bounds_accounts_for_rotated_bearing_and_pitch() {
    let mut view = test_view(1000, 500, LatLon::new(0.0, 0.0), 2.0);
    // Wide east-west strip: fits at higher zoom at bearing 0° than at bearing 90° in a 1000x500 viewport.
    let strip = LatLonBounds::new(-8.0, -1.0, 8.0, 1.0);
    let pad = EdgePadding::uniform(50.0);

    let fit_0 = view.camera_for_bounds(strip, pad, Some(Deg(0.0)), Some(Deg(0.0)));
    let fit_90 = view.camera_for_bounds(strip, pad, Some(Deg(90.0)), Some(Deg(0.0)));
    // Viewport inner width is 900 vs inner height 400 -> rotating 90° reduces zoom by log2(900/400).
    let expected_diff = (900.0_f64 / 400.0_f64).log2();
    assert!(
        ((fit_0.zoom - fit_90.zoom) - expected_diff).abs() < 1e-6,
        "fit_0={} fit_90={} expected_diff={expected_diff}",
        fit_0.zoom,
        fit_90.zoom
    );

    // Verify 45° bearing + 30° pitch keeps all 4 corners strictly inside the padded box!
    let fit_tilted = view.camera_for_bounds(strip, pad, Some(Deg(45.0)), Some(Deg(30.0)));
    view.jump_to(fit_tilted.into()).expect("jump tilted");

    for corner in [
        LatLon::new(strip.north, strip.west),
        LatLon::new(strip.north, strip.east),
        LatLon::new(strip.south, strip.east),
        LatLon::new(strip.south, strip.west),
    ] {
        let p = project_lat_lon_to_window(&view, corner);
        assert!(
            p.x >= pad.left - 1e-4 && p.x <= 1000.0 - pad.right + 1e-4,
            "p={p:?}"
        );
        assert!(
            p.y >= pad.top - 1e-4 && p.y <= 500.0 - pad.bottom + 1e-4,
            "p={p:?}"
        );
    }
}
