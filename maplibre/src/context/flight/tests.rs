#![allow(clippy::expect_used, clippy::panic)]

use std::time::Duration;

use image::{Rgba, RgbaImage};

use super::FlightOutcome;
use crate::{
    coords::{LatLon, WorldTileCoords},
    headless::{create_headless_renderer, map::HeadlessMap, HeadlessPlugin},
    render::{
        view_state::{CameraOptions, FlightCurve, FlyOptions, NavigationMode},
        RenderPlugin,
    },
    terrain::{interaction::camera_ground_position, DefaultDemTransferables, TerrainPlugin},
};

const OCEAN: LatLon = LatLon {
    latitude: -30.0,
    longitude: -20.0,
};
const ORYOL: LatLon = LatLon {
    latitude: 52.97,
    longitude: 36.06,
};
const FRAME: Duration = Duration::from_millis(16);

fn flat_dem(meters: f64) -> RgbaImage {
    let value = meters + 32768.0;
    let red = (value / 256.0).floor();
    RgbaImage::from_pixel(
        256,
        256,
        Rgba([red as u8, (value - red * 256.0) as u8, 0, 255]),
    )
}

fn tile_at(position: LatLon, zoom: u8) -> WorldTileCoords {
    let n = 2_f64.powi(i32::from(zoom));
    let latitude = position.latitude.to_radians();
    let x = (position.longitude + 180.0) / 360.0 * n;
    let y = (1.0 - (latitude.tan() + 1.0 / latitude.cos()).ln() / std::f64::consts::PI) / 2.0 * n;
    WorldTileCoords::from((x.floor() as i32, y.floor() as i32, zoom.into()))
}

/// A free-globe map over terrain whose camera stands on a −5468 m ocean floor.
async fn ocean_map() -> HeadlessMap {
    let style = serde_json::from_value(serde_json::json!({
        "version":8,"center":[OCEAN.longitude, OCEAN.latitude],"zoom":9,"pitch":50,
        "projection":{"type":"vertical-perspective"},"terrain":{"source":"dem"},
        "sources":{"dem":{"type":"raster-dem","tiles":["offline://dem"],
            "tileSize":256,"maxzoom":14,"encoding":"terrarium"}},
        "layers":[]
    }))
    .expect("style");
    let (kernel, renderer) = create_headless_renderer(64, 64, None)
        .await
        .expect("renderer");
    let mut map = HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(TerrainPlugin::<DefaultDemTransferables>::default()),
            Box::new(HeadlessPlugin::new(false)),
        ],
    )
    .expect("map");
    map.set_navigation_mode(NavigationMode::FreeGlobe)
        .expect("free navigation");
    map.load_dem_tiles(vec![(tile_at(OCEAN, 6), flat_dem(-5468.0))])
        .expect("ocean floor");
    map.run_frame().expect("frame");
    map
}

/// Draws the next frame `FRAME` later than the last.
fn frame(map: &mut HeadlessMap) {
    map.frame_input_mut().timestamp += FRAME;
    map.run_frame().expect("frame");
}

fn to_oryol() -> CameraOptions {
    CameraOptions {
        center: Some(ORYOL),
        zoom: Some(14.0),
        pitch: Some(cgmath::Deg(55.0)),
        ..Default::default()
    }
}

#[tokio::test]
async fn a_flight_crosses_above_the_ground_and_arrives_once() {
    let mut map = ocean_map().await;
    let revision = map.context().camera_revision();
    let flight = map.fly_to(
        to_oryol(),
        FlightCurve::Fly(FlyOptions {
            duration: Some(Duration::from_millis(800)),
            ..FlyOptions::default()
        }),
    );
    let mut frames = 0;
    while map.context().flight_outcome(flight) == FlightOutcome::Flying {
        frame(&mut map);
        frames += 1;
        assert!(frames < 100, "the flight never arrives");
        let eye = camera_ground_position(map.view_state()).1;
        let ground = map.view_state().center_elevation();
        assert!(
            eye > ground.max(0.0),
            "frame {frames}: eye at {eye} m over {ground} m"
        );
    }
    assert_eq!(map.context().flight_outcome(flight), FlightOutcome::Arrived);
    assert!(frames >= 800 / 16, "arrived after {frames} frames");
    assert_ne!(map.context().camera_revision(), revision);
    let arrived = map.view_state().center_lat_lon();
    assert!(
        (arrived.latitude - ORYOL.latitude).abs() < 1e-6,
        "{arrived}"
    );
    assert!(
        (arrived.longitude - ORYOL.longitude).abs() < 1e-6,
        "{arrived}"
    );
    // Nothing loaded under Oryol: the center rests at sea level, not on the ocean floor.
    assert_eq!(map.view_state().center_elevation(), 0.0);
    let settled = map.context().camera_revision();
    frame(&mut map);
    frame(&mut map);
    assert_eq!(map.context().flight_outcome(flight), FlightOutcome::Arrived);
    assert_eq!(
        map.context().camera_revision(),
        settled,
        "a still camera keeps its revision"
    );
}

#[tokio::test]
async fn moving_the_camera_any_other_way_interrupts_a_flight() {
    let mut map = ocean_map().await;
    let flight = map.fly_to(to_oryol(), FlightCurve::Fly(FlyOptions::default()));
    frame(&mut map);
    frame(&mut map);
    map.jump_to(CameraOptions {
        center: Some(LatLon::new(10.0, 10.0)),
        ..Default::default()
    })
    .expect("jump");
    frame(&mut map);
    assert_eq!(
        map.context().flight_outcome(flight),
        FlightOutcome::Interrupted
    );
    let center = map.view_state().center_lat_lon();
    assert!((center.latitude - 10.0).abs() < 1e-6, "{center}");
    assert!(!map.view_state().center_elevation_frozen());
}

#[tokio::test]
async fn a_new_flight_interrupts_the_one_under_way() {
    let mut map = ocean_map().await;
    let first = map.fly_to(to_oryol(), FlightCurve::Fly(FlyOptions::default()));
    frame(&mut map);
    let second = map.fly_to(
        CameraOptions {
            zoom: Some(4.0),
            ..Default::default()
        },
        FlightCurve::Ease {
            duration: Duration::from_millis(100),
        },
    );
    assert_ne!(first, second);
    assert_eq!(
        map.context().flight_outcome(first),
        FlightOutcome::Interrupted
    );
    assert_eq!(map.context().flight_outcome(second), FlightOutcome::Flying);
    while map.context().flight_outcome(second) == FlightOutcome::Flying {
        frame(&mut map);
    }
    assert_eq!(map.context().flight_outcome(second), FlightOutcome::Arrived);
    assert_eq!(
        map.context().flight_outcome(first),
        FlightOutcome::Superseded
    );
    assert!((map.view_state().zoom().value() - 4.0).abs() < 1e-9);
}
