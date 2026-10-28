//! A full moon in the Earth's shadow: dim and copper in the umbra, as dark as its Danjon value
//! says, and lit as before where the shadow does not reach.
#![allow(clippy::expect_used, clippy::panic)]

use cgmath::{InnerSpace, Vector3, Vector4};

use crate::{
    celestial::{Disc, EarthShadow, EcefDirection},
    headless::map::{readback::read_blocking, HeadlessMap},
    projection::globe::camera::GlobeCameraState,
    render::{projection::globe_camera_for_view, RenderPlugin},
    style::Style,
};

const SIZE: u32 = 256;
/// Wide enough to resolve the shadow across the disc.
const MOON_RADIUS_DEGREES: f64 = 3.0;
/// A shadow scaled with the moon, keeping the umbra wider than it as the real one is.
const UMBRA_DEGREES: f64 = 4.0;
const PENUMBRA_DEGREES: f64 = 5.0;

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

/// The mean colour over the disc's pixels within `radius` of `(x, y)` whose offset across
/// satisfies `keep`.
fn mean(pixels: &[u8], (x, y): (f64, f64), radius: f64, keep: impl Fn(f64) -> bool) -> [f64; 3] {
    let (mut sum, mut count) = ([0.0; 3], 0);
    for py in 0..SIZE {
        for px in 0..SIZE {
            let (dx, dy) = (f64::from(px) + 0.5 - x, f64::from(py) + 0.5 - y);
            if (dx * dx + dy * dy).sqrt() > radius || !keep(dx) {
                continue;
            }
            let at = ((py * SIZE + px) * 4) as usize;
            for (channel, total) in sum.iter_mut().enumerate() {
                *total += f64::from(pixels[at + channel]);
            }
            count += 1;
        }
    }
    assert!(count > 50, "the disc is drawn");
    sum.map(|total| total / f64::from(count))
}

fn luminance([r, g, b]: [f64; 3]) -> f64 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

#[tokio::test]
async fn a_moon_in_the_umbra_turns_dim_and_copper() {
    let mut map = globe().await;
    let camera = globe_camera_for_view(map.view_state()).expect("camera");
    // In space up and to the right of the globe, clear of its disc.
    let centre = (200.0, 56.0);
    let moon = direction_through(&camera, centre.0, centre.1);
    let radius = 0.8
        * crate::starfield::sun::radius_pixels(
            camera.projection().y.y,
            f64::from(SIZE),
            MOON_RADIUS_DEGREES,
        );
    // A full moon: the sun stands opposite it.
    map.set_sun_direction(Some(ecef(-moon)));
    map.set_moon_disc(Some(
        Disc::new(ecef(moon), MOON_RADIUS_DEGREES).expect("disc"),
    ));
    let whole = |_| true;
    let full = mean(&frame(&mut map), centre, radius, whole);

    let shadow = |axis: Vector3<f64>, danjon| {
        Some(
            EarthShadow::new(ecef(axis), UMBRA_DEGREES, PENUMBRA_DEGREES, Some(danjon))
                .expect("shadow"),
        )
    };
    map.set_earth_shadow(shadow(moon, 2.0));
    let total = mean(&frame(&mut map), centre, radius, whole);
    map.set_earth_shadow(shadow(moon, 0.0));
    let dark = mean(&frame(&mut map), centre, radius, whole);
    map.set_earth_shadow(shadow(moon, 4.0));
    let bright = mean(&frame(&mut map), centre, radius, whole);
    let report = format!("full {full:?}, L2 {total:?}, L0 {dark:?}, L4 {bright:?}");
    assert!(
        luminance(total) < 0.35 * luminance(full),
        "totality dims the moon: {report}"
    );
    assert!(
        total[0] > 1.5 * total[2] && total[0] > total[1],
        "a copper moon: {report}"
    );
    assert!(
        luminance(dark) < luminance(total) && luminance(total) < luminance(bright),
        "Danjon's scale orders the darkness: {report}"
    );

    // The umbra's edge through the moon's centre, the shadow off to its right: the right half
    // in the umbra, the left half's limb in sunlight.
    let pixels_per_degree = radius / MOON_RADIUS_DEGREES;
    let axis = direction_through(
        &camera,
        centre.0 + UMBRA_DEGREES * pixels_per_degree / 0.8,
        centre.1,
    );
    map.set_earth_shadow(shadow(axis, 2.0));
    let partial = frame(&mut map);
    let left = mean(&partial, centre, radius, |dx| dx < -radius * 0.5);
    let right = mean(&partial, centre, radius, |dx| dx > radius * 0.3);
    assert!(
        luminance(right) < 0.4 * luminance(left),
        "the shadowed side is darker: left {left:?}, right {right:?}"
    );
    assert!(
        luminance(left) > 0.8 * luminance(full),
        "the sunlit limb keeps its light: left {left:?}, full {full:?}"
    );
}
