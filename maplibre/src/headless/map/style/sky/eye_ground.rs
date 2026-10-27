//! A free-globe eye standing on a globe without terrain sees the globe's surface below its
//! horizon, not the sky and the stars behind it.
#![allow(clippy::expect_used, clippy::panic)]

use cgmath::{InnerSpace, Matrix, Matrix3, Quaternion, Vector3};

use crate::{
    celestial::{ecef_to_globe_world, EcefDirection},
    headless::map::{readback::read_blocking, HeadlessMap},
    projection::globe::EARTH_RADIUS_METERS,
    render::{
        view_state::{GlobePose, NavigationMode},
        RenderPlugin,
    },
    style::Style,
};

const SIZE: u32 = 256;
const FIELD_OF_VIEW_DEGREES: f64 = 100.0;
/// Madrid.
const LATITUDE: f64 = 40.4168;
const LONGITUDE: f64 = -3.7038;
const EYE_HEIGHT_METERS: f64 = 2.0;
/// How far above the horizon the eye looks.
const LOOK_UP_DEGREES: f64 = 40.0;
const BACKGROUND: [u8; 3] = [0x20, 0x60, 0x40];

/// Up, east and north at the observer, in globe world axes.
fn local_axes() -> (Vector3<f64>, Vector3<f64>, Vector3<f64>) {
    let (latitude, longitude) = (LATITUDE.to_radians(), LONGITUDE.to_radians());
    let up = Vector3::new(
        latitude.cos() * longitude.cos(),
        latitude.cos() * longitude.sin(),
        latitude.sin(),
    );
    let east = Vector3::new(-longitude.sin(), longitude.cos(), 0.0);
    let north = up.cross(east);
    (
        ecef_to_globe_world(up),
        ecef_to_globe_world(east),
        ecef_to_globe_world(north),
    )
}

/// The pose 2 m above the observer looking south, `LOOK_UP_DEGREES` above the horizon, level
/// across the frame.
fn standing_pose() -> GlobePose {
    let (up, _, north) = local_axes();
    let elevation = LOOK_UP_DEGREES.to_radians();
    let forward = -north * elevation.cos() + up * elevation.sin();
    let right = forward.cross(up).normalize();
    let camera_up = right.cross(forward);
    let rotation = Matrix3::from_cols(right, camera_up, -forward).transpose();
    let orientation = Quaternion::from(rotation);
    let eye = up * (EARTH_RADIUS_METERS + EYE_HEIGHT_METERS);
    let distance = 1000.0;
    let target = eye + forward * distance;
    GlobePose {
        target: target.normalize().into(),
        target_elevation_meters: target.magnitude() - EARTH_RADIUS_METERS,
        orientation: [
            orientation.s,
            orientation.v.x,
            orientation.v.y,
            orientation.v.z,
        ],
        distance_meters: distance,
    }
}

async fn standing_view() -> HeadlessMap {
    let style: Style = serde_json::from_str(r##"{"version":8,"center":[-3.7038,40.4168],"zoom":16,"sources":{},"layers":[{"id":"background","type":"background","paint":{"background-color":"#206040"}}],"projection":{"type":"vertical-perspective"}}"##).expect("style");
    let (kernel, renderer) = crate::headless::create_headless_renderer(SIZE, SIZE, None)
        .await
        .expect("renderer");
    let mut map = HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(crate::background::BackgroundPlugin),
            Box::new(crate::starfield::StarfieldPlugin),
        ],
    )
    .expect("map");
    map.set_navigation_mode(NavigationMode::FreeGlobe)
        .expect("free navigation");
    map.set_vertical_field_of_view(cgmath::Deg(FIELD_OF_VIEW_DEGREES));
    map.set_atmosphere(true);
    map.restore_globe_pose(standing_pose()).expect("pose");
    for _ in 0..10 {
        map.run_frame().expect("frame");
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
    }
    map
}

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 3] {
    let at = ((y * SIZE + x) * 4) as usize;
    [pixels[at], pixels[at + 1], pixels[at + 2]]
}

/// Eclipse's case: the eye 2 m above Madrid on a globe without terrain, looking 40° up with a
/// 100° field of view, has the horizon in the frame and the ground below it.
#[tokio::test]
async fn an_eye_on_a_globe_without_terrain_sees_the_ground_below_its_horizon() {
    let mut map = standing_view().await;
    let (up, ..) = local_axes();
    let sun = up * 0.8 - local_axes().2 * 0.6;
    map.set_sun_direction(Some(
        EcefDirection::new(sun.z, sun.x, sun.y).expect("direction"),
    ));
    map.run_frame().expect("frame");
    let pixels = read_blocking(
        &map,
        map.head_texture().expect("head texture"),
        wgpu::TextureAspect::All,
    )
    .expect("readback");
    let half = f64::from(SIZE) / 2.0;
    let focal = half / (FIELD_OF_VIEW_DEGREES / 2.0).to_radians().tan();
    // The horizon lies LOOK_UP_DEGREES below the frame's centre.
    let horizon = half + focal * LOOK_UP_DEGREES.to_radians().tan();
    let rows: Vec<u32> = [8.0, 30.0, 60.0]
        .iter()
        .map(|below| (horizon + below) as u32)
        .filter(|row| *row < SIZE)
        .collect();
    let report = rows
        .iter()
        .map(|&row| format!("row {row}: {:?}", pixel(&pixels, SIZE / 2, row)))
        .collect::<Vec<_>>()
        .join(", ");
    assert!(rows.len() >= 2, "rows below the horizon {horizon:.1}");
    for &row in &rows {
        let ground = pixel(&pixels, SIZE / 2, row);
        let green = i32::from(ground[1]) - i32::from(ground[0]).max(i32::from(ground[2]));
        assert!(
            green > 10,
            "the background's green ground is drawn below the horizon at {horizon:.1}, not the \
             sky ({BACKGROUND:?} lit by the sun): {report}"
        );
    }
}
