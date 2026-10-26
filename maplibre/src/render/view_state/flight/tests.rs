#![allow(clippy::expect_used, clippy::panic)]

use std::time::Duration;

use cgmath::Deg;

use super::{FlightCurve, FlyOptions};
use crate::{
    coords::{LatLon, WorldCoords, Zoom},
    render::view_state::{CameraOptions, ViewState},
    window::PhysicalSize,
};

const ORYOL: LatLon = LatLon {
    latitude: 52.97,
    longitude: 36.06,
};

fn view(center: LatLon, zoom: f64, bearing: f64) -> ViewState {
    let zoom = Zoom::new(zoom);
    let mut view = ViewState::new(
        PhysicalSize::new(1200, 900).expect("size"),
        WorldCoords::from_lat_lon(center, zoom),
        zoom,
        Deg(0.0),
        Deg(36.869_897_645_844_02),
    );
    view.camera_mut().set_bearing(Deg(bearing));
    view
}

fn destination() -> CameraOptions {
    CameraOptions {
        center: Some(ORYOL),
        zoom: Some(14.0),
        bearing: Some(Deg(10.0)),
        pitch: Some(Deg(50.0)),
        ..Default::default()
    }
}

#[test]
fn a_move_starts_where_the_camera_is_and_ends_exactly_at_the_destination() {
    let start = view(LatLon::new(-30.0, -20.0), 5.0, 350.0);
    for curve in [
        FlightCurve::Ease {
            duration: Duration::from_millis(500),
        },
        FlightCurve::Fly(FlyOptions::default()),
    ] {
        let path = start.flight_path(destination(), curve);
        let first = path.at(Duration::ZERO, 0.0, 0.0);
        let center = first.center.expect("center");
        assert!((center.latitude + 30.0).abs() < 1e-9, "{curve:?}: {center}");
        assert!((first.zoom.expect("zoom") - 5.0).abs() < 1e-9, "{curve:?}");
        assert!(!path.arrived(Duration::ZERO));
        let last = path.at(path.duration(), -5468.0, 180.0);
        assert_eq!(last.center, Some(ORYOL), "{curve:?}");
        assert_eq!(last.zoom, Some(14.0));
        assert_eq!(last.elevation, Some(180.0));
        // 10 degrees, reached by turning on from 350.
        assert_eq!(last.bearing, Some(Deg(370.0)));
        assert!(path.arrived(path.duration()));
    }
}

#[test]
fn a_bearing_turns_the_short_way() {
    let path = view(ORYOL, 10.0, 350.0).flight_path(
        CameraOptions {
            bearing: Some(Deg(10.0)),
            ..Default::default()
        },
        FlightCurve::Ease {
            duration: Duration::from_secs(1),
        },
    );
    let middle = path.at(Duration::from_millis(500), 0.0, 0.0);
    let bearing = middle.bearing.expect("bearing").0.rem_euclid(360.0);
    assert!(
        bearing >= 350.0 || bearing <= 10.0,
        "turned the long way: {bearing}"
    );
}

#[test]
fn a_long_flight_zooms_out_to_cross_and_takes_the_speed_it_is_given() {
    let start = view(LatLon::new(-30.0, -20.0), 12.0, 0.0);
    let slow = start.flight_path(destination(), FlightCurve::Fly(FlyOptions::default()));
    let lowest = (0..=100)
        .map(|step| slow.duration().mul_f64(f64::from(step) / 100.0))
        .map(|elapsed| slow.at(elapsed, 0.0, 0.0).zoom.expect("zoom"))
        .fold(f64::INFINITY, f64::min);
    assert!(lowest < 8.0, "the flight stays low: zoom {lowest}");
    let fast = start.flight_path(
        destination(),
        FlightCurve::Fly(FlyOptions {
            speed: 2.4,
            ..FlyOptions::default()
        }),
    );
    let ratio = slow.duration().as_secs_f64() / fast.duration().as_secs_f64();
    assert!((ratio - 2.0).abs() < 1e-9, "{ratio}");
}

#[test]
fn a_flight_in_place_only_zooms() {
    let start = view(ORYOL, 6.0, 0.0);
    let path = start.flight_path(
        CameraOptions {
            zoom: Some(12.0),
            ..Default::default()
        },
        FlightCurve::Fly(FlyOptions::default()),
    );
    assert!(path.duration() > Duration::ZERO);
    let middle = path.at(path.duration() / 2, 0.0, 0.0);
    let center = middle.center.expect("center");
    assert!((center.latitude - ORYOL.latitude).abs() < 1e-9, "{center}");
    let zoom = middle.zoom.expect("zoom");
    assert!(zoom > 6.0 && zoom < 12.0, "{zoom}");
}
