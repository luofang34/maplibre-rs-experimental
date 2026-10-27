//! A full moon is by far the brightest thing in the night sky: a near-white disc well above the
//! brightest star, a halo that falls off with angle and follows its phase, and a sky around it
//! bright enough to hide the stars beside it.
#![allow(clippy::expect_used, clippy::panic)]

use cgmath::{InnerSpace, Vector3, Vector4};

use crate::{
    celestial::{Disc, EcefDirection, Star},
    headless::map::{readback::read_blocking, HeadlessMap},
    projection::globe::camera::GlobeCameraState,
    render::{projection::globe_camera_for_view, view_state::CameraOptions, RenderPlugin},
    style::Style,
};

const SIZE: u32 = 256;
/// The full moon's angular radius, kept physical.
const MOON_RADIUS_DEGREES: f64 = 0.26;
/// Wide enough a view to hold the halo, narrow enough that the disc spans pixels.
const FIELD_OF_VIEW_DEGREES: f64 = 10.0;
/// Where the moon stands in the frame, in the sky above the horizon at mid-height.
const MOON_PIXEL: (f64, f64) = (128.0, 64.0);

/// A night view along the horizon, its upper half sky.
async fn night_sky() -> HeadlessMap {
    let style: Style = serde_json::from_str(r##"{"version":8,"center":[0,0],"zoom":16,"sources":{},"layers":[{"id":"background","type":"background","paint":{"background-color":"#203040"}}],"projection":{"type":"vertical-perspective"}}"##).expect("style");
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
    map.set_max_pitch(cgmath::Deg(90.0));
    map.set_vertical_field_of_view(cgmath::Deg(FIELD_OF_VIEW_DEGREES));
    map.jump_to(CameraOptions {
        pitch: Some(cgmath::Deg(89.9)),
        ..Default::default()
    })
    .expect("jump");
    map.pin_center_elevation(2.0);
    map.run_frame().expect("frame");
    map
}

fn camera(map: &HeadlessMap) -> GlobeCameraState {
    globe_camera_for_view(map.view_state()).expect("camera")
}

/// The globe world direction from the eye through pixel `(x, y)`.
fn direction_through(map: &HeadlessMap, (x, y): (f64, f64)) -> Vector3<f64> {
    let camera = camera(map);
    let ndc = (
        x / f64::from(SIZE) * 2.0 - 1.0,
        1.0 - y / f64::from(SIZE) * 2.0,
    );
    let far = camera.inverse_view_projection() * Vector4::new(ndc.0, ndc.1, 0.5, 1.0);
    (far.truncate() / far.w - camera.camera_position()).normalize()
}

/// The ECEF direction of a globe world direction.
fn ecef(globe: Vector3<f64>) -> EcefDirection {
    EcefDirection::new(globe.z, globe.x, globe.y).expect("direction")
}

/// `direction` turned `degrees` about `axis`.
fn turned(direction: Vector3<f64>, axis: Vector3<f64>, degrees: f64) -> Vector3<f64> {
    let axis = axis.normalize();
    let (sine, cosine) = degrees.to_radians().sin_cos();
    direction * cosine + axis.cross(direction) * sine + axis * axis.dot(direction) * (1.0 - cosine)
}

/// The moon's radius in pixels.
fn moon_radius_pixels(map: &HeadlessMap) -> f64 {
    crate::starfield::sun::radius_pixels(
        camera(map).projection().y.y,
        f64::from(SIZE),
        MOON_RADIUS_DEGREES,
    )
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

/// The brightest channel at pixel `(x, y)`.
fn value(pixels: &[u8], (x, y): (f64, f64)) -> u8 {
    let at = ((y as u32 * SIZE + x as u32) * 4) as usize;
    pixels[at..at + 3].iter().copied().max().unwrap_or(0)
}

/// The brightest channel within `radius` pixels of `(x, y)`, and its summed rise over `plain`.
fn peak_and_light(lit: &[u8], plain: &[u8], (x, y): (f64, f64), radius: f64) -> (u8, u32) {
    let (mut peak, mut light) = (0, 0);
    for py in 0..SIZE {
        for px in 0..SIZE {
            let (dx, dy) = (f64::from(px) + 0.5 - x, f64::from(py) + 0.5 - y);
            if dx * dx + dy * dy > radius * radius {
                continue;
            }
            let at = (f64::from(px), f64::from(py));
            peak = peak.max(value(lit, at));
            light += u32::from(value(lit, at).saturating_sub(value(plain, at)));
        }
    }
    (peak, light)
}

/// A full moon, the sun opposite it below the horizon.
fn full_moon(map: &mut HeadlessMap, moon: Vector3<f64>) {
    map.set_sun_direction(Some(ecef(-moon)));
    map.set_moon_disc(Some(
        Disc::new(ecef(moon), MOON_RADIUS_DEGREES).expect("disc"),
    ));
}

/// The full moon's centre and its light outshine the brightest star, Sirius at -1.46.
#[tokio::test]
async fn a_full_moon_outshines_the_brightest_star() {
    let mut map = night_sky().await;
    let moon = direction_through(&map, MOON_PIXEL);
    let star_pixel = (40.0, 40.0);
    let star = direction_through(&map, star_pixel);
    map.set_sun_direction(Some(ecef(-moon)));
    let plain = frame(&mut map);
    map.set_stars(vec![Star::new(ecef(star), -1.46, None).expect("star")]);
    let starlit = frame(&mut map);
    map.set_stars(Vec::new());
    full_moon(&mut map, moon);
    let moonlit = frame(&mut map);

    let reach = 3.0 * moon_radius_pixels(&map);
    let (star_peak, star_light) = peak_and_light(&starlit, &plain, star_pixel, reach);
    let (moon_peak, moon_light) = peak_and_light(&moonlit, &plain, MOON_PIXEL, reach);
    let centre = value(&moonlit, MOON_PIXEL);
    let report = format!(
        "moon centre {centre}, peak {moon_peak}, light {moon_light}; \
         star peak {star_peak}, light {star_light}; centre ratio {:.2}, light ratio {:.1}",
        f64::from(centre) / f64::from(star_peak),
        f64::from(moon_light) / f64::from(star_light.max(1)),
    );
    assert!(centre >= 230, "the moon is near-white: {report}");
    assert!(
        f64::from(centre) >= 1.2 * f64::from(star_peak),
        "the moon's centre outshines the star's: {report}"
    );
    assert!(
        moon_light >= 10 * star_light,
        "the moon gives far more light: {report}"
    );
}

/// The halo around a full moon falls off with the angle from it, and a crescent's is far
/// weaker than a full moon's.
#[tokio::test]
async fn the_halo_falls_off_with_angle_and_follows_the_phase() {
    let mut map = night_sky().await;
    let moon = direction_through(&map, MOON_PIXEL);
    let radius = moon_radius_pixels(&map);
    map.set_sun_direction(Some(ecef(-moon)));
    let plain = frame(&mut map);
    full_moon(&mut map, moon);
    let full = frame(&mut map);
    // Out from the centre towards the top of the frame, in the sky.
    let rise = |pixels: &[u8], radii: f64| {
        let at = (MOON_PIXEL.0, MOON_PIXEL.1 - radii * radius);
        value(pixels, at).saturating_sub(value(&plain, at))
    };
    let falloff: Vec<u8> = [2.0, 4.0, 8.0]
        .iter()
        .map(|&radii| rise(&full, radii))
        .collect();
    assert!(falloff[0] > 20, "a full moon glows beside it: {falloff:?}");
    assert!(
        falloff.windows(2).all(|pair| pair[1] < pair[0]),
        "the halo falls off with angle: {falloff:?}"
    );

    // A crescent, 30° from the sun, phase angle 150°.
    let up = camera(&map).camera_position().normalize();
    let sun = turned(moon, moon.cross(up), 30.0);
    map.set_sun_direction(Some(ecef(sun)));
    let crescent = frame(&mut map);
    let thin = rise(&crescent, 2.0);
    assert!(
        u32::from(thin) * 10 < u32::from(falloff[0]),
        "a crescent's halo {thin} is far weaker than a full moon's {}",
        falloff[0]
    );
    let (_, crescent_light) = peak_and_light(&crescent, &plain, MOON_PIXEL, radius);
    let (_, full_light) = peak_and_light(&full, &plain, MOON_PIXEL, radius);
    assert!(
        crescent_light * 3 < full_light,
        "a crescent's disc {crescent_light} is dimmer than a full moon's {full_light}"
    );
}

/// A star 3° from a full moon is washed out by the sky the moon lights; the same star with the
/// moon 40° away shows.
#[tokio::test]
async fn stars_beside_a_full_moon_fade() {
    let mut map = night_sky().await;
    let moon = direction_through(&map, MOON_PIXEL);
    let up = camera(&map).camera_position().normalize();
    let star = turned(moon, up, 3.0);
    let star_pixel = {
        let clip = camera(&map).view_projection() * star.extend(0.0);
        (
            (clip.x / clip.w + 1.0) / 2.0 * f64::from(SIZE),
            (1.0 - clip.y / clip.w) / 2.0 * f64::from(SIZE),
        )
    };
    let mut rises = Vec::new();
    for moon in [moon, turned(moon, up, 40.0)] {
        full_moon(&mut map, moon);
        map.set_stars(Vec::new());
        let plain = frame(&mut map);
        map.set_stars(vec![Star::new(ecef(star), 1.0, None).expect("star")]);
        let lit = frame(&mut map);
        rises.push(value(&lit, star_pixel).saturating_sub(value(&plain, star_pixel)));
    }
    let [near, far] = rises[..] else {
        panic!("two frames");
    };
    assert!(
        far > 150,
        "the star shows away from the moon: {far} at {star_pixel:?}"
    );
    assert!(
        near * 3 < far,
        "beside the moon it fades: {near} against {far}"
    );
}
