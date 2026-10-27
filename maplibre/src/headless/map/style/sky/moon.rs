//! The moon's phase follows the sun and moon directions alone: the half facing the sun is lit.
#![allow(clippy::expect_used, clippy::panic)]

use cgmath::{InnerSpace, Vector3, Vector4};

use crate::{
    celestial::{Disc, EcefDirection},
    headless::map::{readback::read_blocking, HeadlessMap},
    projection::globe::camera::GlobeCameraState,
    render::{projection::globe_camera_for_view, RenderPlugin},
    style::Style,
};

const SIZE: u32 = 256;
/// Wide enough to resolve the phase, about twenty pixels across in this view.
const MOON_RADIUS_DEGREES: f64 = 3.0;

async fn globe() -> HeadlessMap {
    let style: Style = serde_json::from_str(r##"{"version":8,"center":[0,0],"zoom":0,"sources":{},"layers":[{"id":"background","type":"background","paint":{"background-color":"#303030"}}],"projection":{"type":"globe"}}"##).expect("style");
    let (kernel, renderer) = crate::headless::create_headless_renderer(SIZE, SIZE, None)
        .await
        .expect("renderer");
    HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(crate::background::BackgroundPlugin),
            Box::new(crate::starfield::StarfieldPlugin),
        ],
    )
    .expect("map")
}

fn frame(map: &mut HeadlessMap) -> Vec<u8> {
    map.run_frame().expect("frame");
    read_blocking(
        map,
        map.head_texture().expect("head texture"),
        wgpu::TextureAspect::All,
    )
    .expect("readback")
}

fn camera(map: &HeadlessMap) -> GlobeCameraState {
    globe_camera_for_view(map.view_state()).expect("camera")
}

/// The ECEF direction of a globe world direction, undoing `ecef_to_globe_world`.
fn ecef(globe: Vector3<f64>) -> EcefDirection {
    EcefDirection::new(globe.z, globe.x, globe.y).expect("direction")
}

/// The globe world direction from the eye through pixel `(x, y)`.
fn direction_through(camera: &GlobeCameraState, x: f64, y: f64) -> Vector3<f64> {
    let ndc = (
        x / f64::from(SIZE) * 2.0 - 1.0,
        1.0 - y / f64::from(SIZE) * 2.0,
    );
    let far = camera.inverse_view_projection() * Vector4::new(ndc.0, ndc.1, 0.5, 1.0);
    (far.truncate() / far.w - camera.camera_position()).normalize()
}

/// The mean brightest channel over the disc's left and right halves around `(x, y)`, keeping
/// inside `radius` pixels.
fn halves(pixels: &[u8], (x, y): (f64, f64), radius: f64) -> (f64, f64) {
    let (mut left, mut right) = ((0.0, 0), (0.0, 0));
    for py in 0..SIZE {
        for px in 0..SIZE {
            let (dx, dy) = (f64::from(px) + 0.5 - x, f64::from(py) + 0.5 - y);
            if (dx * dx + dy * dy).sqrt() > radius || dx.abs() < 2.0 {
                continue;
            }
            let at = ((py * SIZE + px) * 4) as usize;
            let value = f64::from(pixels[at..at + 3].iter().copied().max().unwrap_or(0));
            let half = if dx < 0.0 { &mut left } else { &mut right };
            half.0 += value;
            half.1 += 1;
        }
    }
    assert!(left.1 > 50 && right.1 > 50, "the disc covers both halves");
    (left.0 / f64::from(left.1), right.0 / f64::from(right.1))
}

#[tokio::test]
async fn the_half_of_the_moon_facing_the_sun_is_lit() {
    let mut map = globe().await;
    let camera = camera(&map);
    // In space up and to the right of the globe, clear of its disc.
    let centre = (210.0, 50.0);
    let moon = direction_through(&camera, centre.0, centre.1);
    // A quarter turn from the moon, towards the right of the frame: a half moon lit on the right.
    let towards = direction_through(&camera, 250.0, centre.1);
    let sun = (towards - moon * moon.dot(towards)).normalize();
    let radius = 0.8
        * crate::starfield::sun::radius_pixels(
            camera.projection().y.y,
            f64::from(SIZE),
            MOON_RADIUS_DEGREES,
        );
    map.set_moon_disc(Some(
        Disc::new(ecef(moon), MOON_RADIUS_DEGREES).expect("disc"),
    ));

    let mut lit = Vec::new();
    for sun in [Some(sun), Some(-sun), None] {
        map.set_sun_direction(sun.map(ecef));
        lit.push(halves(&frame(&mut map), centre, radius));
    }
    let [(left, right), (left_turned, right_turned), (left_full, right_full)] = lit[..] else {
        panic!("three frames");
    };
    assert!(right > 3.0 * left, "the sun's side is lit: {lit:?}");
    assert!(left > 0.0, "the night side keeps its earthshine: {lit:?}");
    assert!(
        left_turned > 3.0 * right_turned,
        "turning the sun turns the phase: {lit:?}"
    );
    assert!(
        left_full > right * 0.9 && (left_full - right_full).abs() < 10.0,
        "with no sun the whole disc is lit: {lit:?}"
    );
}
