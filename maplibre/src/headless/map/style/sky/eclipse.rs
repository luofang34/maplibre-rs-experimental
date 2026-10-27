//! A solar eclipse seen by an eye on the ground: as the moon covers the sun, the sky darkens
//! overhead while the horizon keeps the glow of the air beyond the shadow, the ground dims to
//! its night, the stars come out, and the moon's disc hides the part of the sun it covers.
#![allow(clippy::expect_used, clippy::panic)]

use cgmath::{InnerSpace, Matrix, Matrix3, Quaternion, Vector3};

use crate::{
    celestial::{ecef_to_globe_world, AngularRadius, Disc, EcefDirection, Star, VisibleFraction},
    headless::map::{readback::read_blocking, HeadlessMap},
    projection::globe::EARTH_RADIUS_METERS,
    render::{
        view_state::{GlobePose, NavigationMode},
        RenderPlugin,
    },
    style::Style,
};

const SIZE: u32 = 256;
const LATITUDE: f64 = 40.4168;
const LONGITUDE: f64 = -3.7038;
/// The sun stands due south, this high.
const SUN_ELEVATION: f64 = 30.0;

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

/// The direction due south at `elevation` degrees, in globe world axes.
fn south_at(elevation: f64) -> Vector3<f64> {
    let (up, _, north) = local_axes();
    let elevation = elevation.to_radians();
    -north * elevation.cos() + up * elevation.sin()
}

/// The direction `east_degrees` to the right of the sun, as the frame shows it.
fn beside_sun(east_degrees: f64) -> Vector3<f64> {
    let (_, east, _) = local_axes();
    let offset = east_degrees.to_radians();
    (south_at(SUN_ELEVATION) * offset.cos() - east * offset.sin()).normalize()
}

fn ecef(globe: Vector3<f64>) -> EcefDirection {
    EcefDirection::new(globe.z, globe.x, globe.y).expect("direction")
}

/// The eye 2 m up looking along `forward`, level across the frame.
fn pose(forward: Vector3<f64>) -> GlobePose {
    let (up, ..) = local_axes();
    let right = forward.cross(up).normalize();
    let rotation = Matrix3::from_cols(right, right.cross(forward), -forward).transpose();
    let orientation = Quaternion::from(rotation);
    let eye = up * (EARTH_RADIUS_METERS + 2.0);
    let target = eye + forward * 1000.0;
    GlobePose {
        target: target.normalize().into(),
        target_elevation_meters: target.magnitude() - EARTH_RADIUS_METERS,
        orientation: [
            orientation.s,
            orientation.v.x,
            orientation.v.y,
            orientation.v.z,
        ],
        distance_meters: 1000.0,
    }
}

async fn standing_view(field_of_view: f64, forward: Vector3<f64>) -> HeadlessMap {
    let style: Style = serde_json::from_str(r##"{"version":8,"center":[-3.7038,40.4168],"zoom":16,"sources":{"dem":{"type":"raster-dem","tiles":["https://dem.example/{z}/{x}/{y}.png"],"encoding":"terrarium"}},"layers":[{"id":"background","type":"background","paint":{"background-color":"#c0c0c0"}}],"terrain":{"source":"dem"},"projection":{"type":"vertical-perspective"}}"##).expect("style");
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
    map.set_navigation_mode(NavigationMode::FreeGlobe)
        .expect("free navigation");
    map.set_vertical_field_of_view(cgmath::Deg(field_of_view));
    map.set_atmosphere(true);
    map.set_sun_direction(Some(ecef(south_at(SUN_ELEVATION))));
    map.restore_globe_pose(pose(forward)).expect("pose");
    for _ in 0..40 {
        map.run_frame().expect("frame");
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
    }
    map
}

fn frame(map: &mut HeadlessMap, fraction: f32) -> Vec<u8> {
    map.set_sun_visible_fraction(VisibleFraction::new(fraction).expect("fraction"));
    map.run_frame().expect("frame");
    read_blocking(
        map,
        map.head_texture().expect("head texture"),
        wgpu::TextureAspect::All,
    )
    .expect("readback")
}

fn luminance(pixels: &[u8], x: u32, y: u32) -> f64 {
    let at = ((y * SIZE + x) * 4) as usize;
    0.2126 * f64::from(pixels[at])
        + 0.7152 * f64::from(pixels[at + 1])
        + 0.0722 * f64::from(pixels[at + 2])
}

/// The row of the frame `degrees` above its centre, for a vertical field of view.
fn row_above_centre(field_of_view: f64, degrees: f64) -> u32 {
    let half = f64::from(SIZE) / 2.0;
    let focal = half / (field_of_view / 2.0).to_radians().tan();
    (half - focal * degrees.to_radians().tan()).round() as u32
}

/// Looking south 30° up with a 90° view, from full sun to totality: the sky overhead darkens
/// to a small part of its daylight, the horizon stays brighter than the sky above it, and the
/// ground keeps its night light, neither its day light nor black.
#[tokio::test]
async fn totality_darkens_the_sky_and_the_ground_and_leaves_the_horizon_glowing() {
    const VIEW: f64 = 90.0;
    let mut map = standing_view(VIEW, south_at(SUN_ELEVATION)).await;
    let x = SIZE / 4;
    let high = row_above_centre(VIEW, 40.0);
    let low = row_above_centre(VIEW, -SUN_ELEVATION + 3.0);
    let ground = row_above_centre(VIEW, -SUN_ELEVATION - 10.0);
    let day = frame(&mut map, 1.0);
    let total = frame(&mut map, 0.0);
    let report = format!(
        "sky at 70° {:.0} → {:.0}, at 3° {:.0} → {:.0}, ground {:.0} → {:.0}",
        luminance(&day, x, high),
        luminance(&total, x, high),
        luminance(&day, x, low),
        luminance(&total, x, low),
        luminance(&day, x, ground),
        luminance(&total, x, ground),
    );
    assert!(luminance(&day, x, high) > 100.0, "a daylit sky: {report}");
    assert!(
        luminance(&total, x, high) < 0.25 * luminance(&day, x, high),
        "the sky overhead darkens: {report}"
    );
    assert!(
        luminance(&total, x, low) > 1.5 * luminance(&total, x, high),
        "the horizon glows above the sky over it: {report}"
    );
    assert!(
        luminance(&total, x, ground) < 0.6 * luminance(&day, x, ground),
        "the ground dims: {report}"
    );
    assert!(
        luminance(&total, x, ground) > 0.2 * luminance(&day, x, ground),
        "to its night light, not black: {report}"
    );
}

/// A bright star 20° from the sun is lost in the day sky and shows in totality.
#[tokio::test]
async fn the_stars_come_out_in_totality() {
    const VIEW: f64 = 90.0;
    let mut map = standing_view(VIEW, south_at(SUN_ELEVATION)).await;
    let star = south_at(SUN_ELEVATION + 20.0);
    map.set_stars(vec![Star::new(ecef(star), -1.0, None).expect("star")]);
    let row = row_above_centre(VIEW, 20.0);
    let column = SIZE / 2;
    let (day, total) = (frame(&mut map, 1.0), frame(&mut map, 0.0));
    let around = |pixels: &[u8]| luminance(pixels, column + 12, row);
    let contrast = |pixels: &[u8]| luminance(pixels, column, row) - around(pixels);
    assert!(
        contrast(&total) > 40.0 && contrast(&total) > contrast(&day) + 30.0,
        "the star shows against the dark sky: {:.0} by day, {:.0} in totality",
        contrast(&day),
        contrast(&total)
    );
}

/// With nine tenths of the sun covered, the moon's disc hides what it covers of the sun's disc
/// and glare, while the uncovered crescent still shines.
#[tokio::test]
async fn the_moon_hides_the_part_of_the_sun_it_covers() {
    const VIEW: f64 = 20.0;
    const SUN_RADIUS: f64 = 1.0;
    let mut map = standing_view(VIEW, south_at(SUN_ELEVATION)).await;
    map.set_sun_angular_radius(Some(AngularRadius::new(SUN_RADIUS).expect("radius")));
    let half = f64::from(SIZE) / 2.0;
    let focal = half / (VIEW / 2.0).to_radians().tan();
    let column_at = |degrees: f64| (half + focal * degrees.to_radians().tan()).round() as u32;
    let row = SIZE / 2;
    let uncovered = frame(&mut map, 1.0);
    // The moon stands 0.6° right of the sun's centre, a little wider than the sun, and leaves a
    // crescent on the sun's left.
    map.set_moon_disc(Some(Disc::new(ecef(beside_sun(0.6)), 1.05).expect("disc")));
    let covered = frame(&mut map, 0.1);
    let (centre, crescent) = (column_at(0.0), column_at(-0.8));
    let report = format!(
        "centre {:.0} → {:.0}, crescent {:.0} → {:.0}",
        luminance(&uncovered, centre, row),
        luminance(&covered, centre, row),
        luminance(&uncovered, crescent, row),
        luminance(&covered, crescent, row),
    );
    assert!(
        luminance(&uncovered, centre, row) > 230.0,
        "the sun: {report}"
    );
    assert!(
        luminance(&covered, centre, row) < 0.6 * luminance(&uncovered, centre, row),
        "the moon covers the sun's centre: {report}"
    );
    assert!(
        luminance(&covered, crescent, row) > 230.0,
        "the crescent still shines: {report}"
    );
}
