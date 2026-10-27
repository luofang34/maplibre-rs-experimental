//! The sun's disc stands on the sea horizon at sunrise: the ground hides the part of it below
//! the horizon the eye sees, and the glare follows the part that shows.
#![allow(clippy::expect_used, clippy::panic)]

use cgmath::{InnerSpace, Vector3};

use crate::{
    celestial::{AngularRadius, EcefDirection},
    headless::map::{readback::read_blocking, HeadlessMap},
    projection::globe::{camera::GlobeCameraState, EARTH_RADIUS_METERS},
    render::{projection::globe_camera_for_view, view_state::CameraOptions, RenderPlugin},
    style::Style,
};

const SIZE: u32 = 256;
/// The sun on 2026-06-14, whose centre crosses the geometric horizon of 41.07204° N,
/// 71.85733° W at 09:19:39.647Z.
const SUNRISE: [f64; 3] = [0.7021538141693241, 0.5923785241370332, 0.39505405375555974];
const SUN_RADIUS_DEGREES: f64 = 0.2626;
const AZIMUTH: f64 = 58.397;

async fn horizon_view() -> HeadlessMap {
    let style: Style = serde_json::from_str(r##"{"version":8,"center":[-71.85733,41.07204],"zoom":18,"sources":{"dem":{"type":"raster-dem","tiles":["https://dem.example/{z}/{x}/{y}.png"],"encoding":"terrarium"}},"layers":[{"id":"background","type":"background","paint":{"background-color":"#204060"}}],"terrain":{"source":"dem"},"projection":{"type":"vertical-perspective"}}"##).expect("style");
    let (kernel, renderer) = crate::headless::create_headless_renderer_with_loader(
        SIZE,
        SIZE,
        Default::default(),
        crate::io::resource_loader::SharedLoader::new(super::daylight::FlatDem),
    )
    .await
    .expect("renderer");
    let mut map = HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(crate::background::BackgroundPlugin),
            Box::new(crate::terrain::TerrainPlugin::<
                crate::terrain::DefaultDemTransferables,
            >::default()),
            Box::new(crate::starfield::StarfieldPlugin),
        ],
    )
    .expect("map");
    map.set_max_pitch(cgmath::Deg(90.0));
    // A narrow view makes the sun about thirteen pixels across.
    map.view_state_mut()
        .set_field_of_view(cgmath::Deg(5.0).into());
    map.set_sun_direction(Some(sun()));
    map
}

fn radius() -> AngularRadius {
    AngularRadius::new(SUN_RADIUS_DEGREES).expect("radius")
}

fn sun() -> EcefDirection {
    EcefDirection::new(SUNRISE[0], SUNRISE[1], SUNRISE[2]).expect("sun")
}

fn camera(map: &HeadlessMap) -> GlobeCameraState {
    globe_camera_for_view(map.view_state()).expect("camera")
}

/// The eye's height above the sea-level sphere, in metres.
fn eye_altitude(map: &HeadlessMap) -> f64 {
    (camera(map).camera_position().magnitude() - 1.0) * EARTH_RADIUS_METERS
}

/// Looks along the sunrise azimuth, level with the eye, from `altitude` metres up.
async fn look_from(map: &mut HeadlessMap, altitude: f64) {
    let mut pinned = altitude;
    for _ in 0..4 {
        map.jump_to(CameraOptions {
            bearing: Some(cgmath::Deg(AZIMUTH)),
            pitch: Some(cgmath::Deg(89.9)),
            ..Default::default()
        })
        .expect("jump");
        map.pin_center_elevation(pinned);
        // The terrain decides on a frame that the globe orbits the pinned centre.
        map.run_frame().expect("frame");
        pinned += altitude - eye_altitude(map);
    }
    assert!(
        (eye_altitude(map) - altitude).abs() < altitude * 0.05,
        "eye at {} m",
        eye_altitude(map)
    );
    // Let the terrain under the view load.
    for _ in 0..40 {
        map.run_frame().expect("frame");
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
    }
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

/// Where the sun's centre falls in the frame and its radius in pixels.
fn sun_in_frame(map: &HeadlessMap) -> ((f64, f64), f64) {
    disc_in_frame(map, sun())
}

/// Where a disc of the sun's radius along `direction` falls in the frame, and its radius.
fn disc_in_frame(map: &HeadlessMap, direction: EcefDirection) -> ((f64, f64), f64) {
    let camera = camera(map);
    let clip = camera.view_projection() * direction.globe_world().extend(0.0);
    assert!(clip.w > 0.0, "the sun lies ahead");
    let centre = (
        (clip.x / clip.w + 1.0) / 2.0 * f64::from(SIZE),
        (1.0 - clip.y / clip.w) / 2.0 * f64::from(SIZE),
    );
    let radius = crate::starfield::sun::radius_pixels(
        camera.projection().y.y,
        f64::from(SIZE),
        SUN_RADIUS_DEGREES,
    );
    (centre, radius)
}

/// The share of the pixels inside the sun's disc, a pixel short of its limb, that the disc
/// brightened.
fn disc_share(without: &[u8], with: &[u8], (centre, radius): ((f64, f64), f64)) -> f64 {
    let (mut inside, mut lit) = (0, 0);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (dx, dy) = (f64::from(x) + 0.5 - centre.0, f64::from(y) + 0.5 - centre.1);
            if (dx * dx + dy * dy).sqrt() > radius - 1.0 {
                continue;
            }
            inside += 1;
            let at = ((y * SIZE + x) * 4) as usize;
            let rise = (at..at + 3)
                .map(|channel| with[channel].saturating_sub(without[channel]))
                .max()
                .unwrap_or(0);
            lit += u32::from(rise > 60);
        }
    }
    assert!(inside > 100, "the disc covers {inside} pixels");
    f64::from(lit) / f64::from(inside)
}

/// The geometric horizon dips below the eye's level by acos(R / (R + h)); the disc's centre
/// at 0° geometric elevation stands that far above the sea horizon the eye sees. No refraction
/// is modelled.
fn horizon_dip_degrees(altitude: f64) -> f64 {
    (EARTH_RADIUS_METERS / (EARTH_RADIUS_METERS + altitude))
        .acos()
        .to_degrees()
}

async fn share_from(altitude: f64) -> (f64, f64) {
    let mut map = horizon_view().await;
    look_from(&mut map, altitude).await;
    let without = frame(&mut map);
    map.set_sun_angular_radius(Some(radius()));
    let with = frame(&mut map);
    let share = disc_share(&without, &with, sun_in_frame(&map));
    (share, horizon_dip_degrees(altitude))
}

/// The sun's direction `degrees` lower than at sunrise, towards the observer's nadir.
fn lowered_sun(degrees: f64) -> EcefDirection {
    let (latitude, longitude) = (41.07204_f64.to_radians(), (-71.85733_f64).to_radians());
    let up = Vector3::new(
        latitude.cos() * longitude.cos(),
        latitude.cos() * longitude.sin(),
        latitude.sin(),
    );
    let sunrise = Vector3::from(SUNRISE);
    let (sine, cosine) = degrees.to_radians().sin_cos();
    let lowered = sunrise * cosine - up * sine;
    EcefDirection::new(lowered.x, lowered.y, lowered.z).expect("sun")
}

/// The glare's rise two disc radii above the disc's centre, in the sky.
async fn glare_from(altitude: f64, sun: EcefDirection) -> u8 {
    let mut map = horizon_view().await;
    look_from(&mut map, altitude).await;
    map.set_sun_direction(Some(sun));
    let without = frame(&mut map);
    map.set_sun_angular_radius(Some(radius()));
    let with = frame(&mut map);
    let clip = camera(&map).view_projection() * sun.globe_world().extend(0.0);
    let (x, y) = (
        (clip.x / clip.w + 1.0) / 2.0 * f64::from(SIZE),
        (1.0 - clip.y / clip.w) / 2.0 * f64::from(SIZE),
    );
    let radius = sun_in_frame(&map).1;
    let at = (((y - 2.0 * radius) as u32 * SIZE + x as u32) * 4) as usize;
    (at..at + 3)
        .map(|channel| with[channel].saturating_sub(without[channel]))
        .max()
        .unwrap_or(0)
}

/// The glare follows the share of the disc the ground leaves showing: full over a whole disc,
/// less over half of one, and none once the ground hides it.
#[tokio::test]
async fn the_glare_follows_the_share_of_the_sun_that_shows() {
    let whole = glare_from(100.0, sun()).await;
    let half = glare_from(1.7, sun()).await;
    let hidden = glare_from(100.0, lowered_sun(1.0)).await;
    assert!(whole > 10, "a whole sun glares: {whole}");
    assert!(
        half < whole && half > whole / 4,
        "half a sun glares less: {half} of {whole}"
    );
    assert!(hidden <= 1, "a hidden sun does not glare: {hidden}");
}

/// From a standing eye, 1.7 m up, the sea horizon dips 0.04°, so a little over half of the
/// disc whose centre is on the geometric horizon shows.
#[tokio::test]
async fn a_standing_eye_sees_about_half_the_rising_sun() {
    let (share, dip) = share_from(1.7).await;
    assert!(dip < 0.05, "dip {dip}°");
    assert!((0.4..=0.75).contains(&share), "{share} of the disc shows");
}

/// From 100 m up, the sea horizon dips 0.32°, more than the sun's radius, so the whole disc
/// stands above it, its lower limb just clear.
#[tokio::test]
async fn from_a_hundred_metres_the_whole_rising_sun_shows() {
    let (share, dip) = share_from(100.0).await;
    assert!(
        dip > SUN_RADIUS_DEGREES && dip < SUN_RADIUS_DEGREES + 0.1,
        "dip {dip}°"
    );
    assert!(share > 0.95, "{share} of the disc shows");
}

/// The sunrise direction turned `degrees` clockwise about the observer's up, along the horizon.
fn turned_sun(degrees: f64) -> EcefDirection {
    let (latitude, longitude) = (41.07204_f64.to_radians(), (-71.85733_f64).to_radians());
    let up = Vector3::new(
        latitude.cos() * longitude.cos(),
        latitude.cos() * longitude.sin(),
        latitude.sin(),
    );
    let sunrise = Vector3::from(SUNRISE);
    // Clockwise seen from above, as azimuth runs, is a negative turn about up.
    let (sine, cosine) = (-degrees).to_radians().sin_cos();
    let turned =
        sunrise * cosine + up.cross(sunrise) * sine + up * up.dot(sunrise) * (1.0 - cosine);
    EcefDirection::new(turned.x, turned.y, turned.z).expect("sun")
}

/// The disc stands where the sun direction points: turning the sun alone moves the disc, so
/// the disc cannot part from the light it stands for.
#[tokio::test]
async fn turning_the_sun_direction_moves_its_disc() {
    let mut map = horizon_view().await;
    look_from(&mut map, 100.0).await;
    let (before, after) = (sun(), turned_sun(1.0));
    let mut shares = Vec::new();
    for direction in [before, after] {
        map.set_sun_direction(Some(direction));
        map.set_sun_angular_radius(None);
        let without = frame(&mut map);
        map.set_sun_angular_radius(Some(radius()));
        let with = frame(&mut map);
        shares.push((
            disc_share(&without, &with, disc_in_frame(&map, before)),
            disc_share(&without, &with, disc_in_frame(&map, after)),
        ));
    }
    assert!(
        shares[0].0 > 0.95,
        "the disc shows where the sun first points: {shares:?}"
    );
    assert!(
        shares[1].1 > 0.95,
        "the disc follows the turned sun: {shares:?}"
    );
    assert!(
        shares[1].0 < 0.05,
        "and leaves where the sun was: {shares:?}"
    );
}
