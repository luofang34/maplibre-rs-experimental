//! A globe without atmosphere, drawn where the host asked for an opaque environment, has an
//! analytic sky: horizon colour on the limb easing to the sky colour above it, the same at
//! every heading and altitude, and the background's ground below the limb.
#![allow(clippy::expect_used, clippy::panic)]

use cgmath::{InnerSpace, Matrix, Matrix3, Quaternion, Vector3};

use crate::{
    celestial::ecef_to_globe_world,
    headless::map::{readback::read_blocking, HeadlessMap},
    projection::globe::EARTH_RADIUS_METERS,
    render::{
        view_state::{GlobePose, NavigationMode},
        RenderPlugin,
    },
    style::Style,
};

const SIZE: u32 = 128;
const FIELD_OF_VIEW_DEGREES: f64 = 60.0;
const LATITUDE: f64 = 39.0;
const LONGITUDE: f64 = -75.5;
const SKY: [u8; 3] = [0x0c, 0x3b, 0x75];
const GROUND: [u8; 3] = [0x3b, 0x2d, 0x1f];
const STYLE: &str = r##"{"version":8,"sources":{},
    "layers":[{"id":"ground","type":"background","paint":{"background-color":"#3b2d1f"}}],
    "sky":{"sky-color":"#0c3b75","horizon-color":"#2c6db8","sky-horizon-blend":0.55},
    "projection":{"type":"vertical-perspective"}}"##;

/// A level eye `height` metres above the observer facing `heading` degrees from north.
fn level_pose(height: f64, heading: f64) -> GlobePose {
    let (latitude, longitude) = (LATITUDE.to_radians(), LONGITUDE.to_radians());
    let up = Vector3::new(
        latitude.cos() * longitude.cos(),
        latitude.cos() * longitude.sin(),
        latitude.sin(),
    );
    let east = Vector3::new(-longitude.sin(), longitude.cos(), 0.0);
    let north = up.cross(east);
    let (up, east, north) = (
        ecef_to_globe_world(up),
        ecef_to_globe_world(east),
        ecef_to_globe_world(north),
    );
    let (sin, cos) = heading.to_radians().sin_cos();
    let forward = north * cos + east * sin;
    let right = forward.cross(up).normalize();
    let camera_up = right.cross(forward);
    let orientation = Quaternion::from(Matrix3::from_cols(right, camera_up, -forward).transpose());
    let eye = up * (EARTH_RADIUS_METERS + height);
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

async fn render(pose: GlobePose, opaque: bool) -> Vec<u8> {
    let style: Style = serde_json::from_str(STYLE).expect("style");
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
        ],
    )
    .expect("map");
    map.set_navigation_mode(NavigationMode::FreeGlobe)
        .expect("free navigation");
    map.set_vertical_field_of_view(cgmath::Deg(FIELD_OF_VIEW_DEGREES));
    map.restore_globe_pose(pose).expect("pose");
    map.set_opaque_environment(opaque);
    for _ in 0..3 {
        map.run_frame().expect("frame");
    }
    read_blocking(
        &map,
        map.head_texture().expect("head texture"),
        wgpu::TextureAspect::All,
    )
    .expect("readback")
}

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let at = ((y * SIZE + x) * 4) as usize;
    [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
}

fn top_quarter(pixels: &[u8]) -> &[u8] {
    &pixels[..(SIZE * SIZE) as usize]
}

fn assert_near(actual: [u8; 4], expected: [u8; 3], tolerance: u8, what: &str) {
    let close = actual
        .iter()
        .zip(expected)
        .all(|(a, e)| a.abs_diff(e) <= tolerance);
    assert!(
        close && actual[3] == 255,
        "{what}: {actual:?} vs {expected:?}"
    );
}

#[test]
fn a_level_eye_sees_sky_above_the_limb_and_ground_below_it() {
    crate::headless::map::test_async::run_async(async {
        let pixels = render(level_pose(450.0, 135.0), true).await;
        assert_near(
            pixel(&pixels, SIZE / 2, 2),
            SKY,
            6,
            "zenith side of the frame",
        );
        assert_near(
            pixel(&pixels, SIZE / 2, SIZE - 3),
            GROUND,
            2,
            "below the limb",
        );
        let horizon_band = pixel(&pixels, SIZE / 2, SIZE / 2 - 2);
        assert!(
            horizon_band[2] > SKY[2] + 30 && horizon_band[3] == 255,
            "the limb takes the brighter horizon colour: {horizon_band:?}"
        );
        for row in 0..SIZE / 2 - 1 {
            let [r, g, b, a] = pixel(&pixels, SIZE / 4, row);
            assert!(
                a == 255 && b > 90 && b > r + 40 && g > r,
                "sky above the limb at row {row}: {:?}",
                [r, g, b, a]
            );
        }
    })
}

#[test]
fn the_sky_is_the_same_at_every_heading() {
    crate::headless::map::test_async::run_async(async {
        let south_east = render(level_pose(450.0, 135.0), true).await;
        let north_west = render(level_pose(450.0, 315.0), true).await;
        let worst = top_quarter(&south_east)
            .iter()
            .zip(top_quarter(&north_west))
            .map(|(a, b)| a.abs_diff(*b))
            .max();
        assert!(worst <= Some(1), "upper sky differs by {worst:?} LSB");
    })
}

#[test]
fn near_space_keeps_the_sky_blue_and_lowers_the_limb() {
    crate::headless::map::test_async::run_async(async {
        let low = render(level_pose(450.0, 90.0), true).await;
        let high = render(level_pose(28_000.0, 90.0), true).await;
        for (x, y) in [(SIZE / 2, 2), (4, 10), (SIZE - 5, SIZE / 4)] {
            for pixels in [&low, &high] {
                let [r, _, b, a] = pixel(pixels, x, y);
                assert!(a == 255 && b > 90 && b > r + 40, "blue at ({x},{y})");
            }
        }
        // 28 km up the limb dips about 5.4° below the eye's horizontal; at 450 m it barely moves.
        let below_centre = SIZE / 2 + 8;
        let [r, _, b, _] = pixel(&high, SIZE / 2, below_centre);
        assert!(b > r + 40, "sky above the dipped limb at 28 km");
        assert_near(
            pixel(&low, SIZE / 2, below_centre),
            GROUND,
            2,
            "450 m ground",
        );
    })
}

#[test]
fn without_an_opaque_environment_the_globe_keeps_its_transparent_space() {
    crate::headless::map::test_async::run_async(async {
        let pixels = render(level_pose(450.0, 135.0), false).await;
        assert_eq!(pixel(&pixels, SIZE / 2, 2)[3], 0, "no sky was requested");
        assert_near(pixel(&pixels, SIZE / 2, SIZE - 3), GROUND, 2, "ground");
    })
}
