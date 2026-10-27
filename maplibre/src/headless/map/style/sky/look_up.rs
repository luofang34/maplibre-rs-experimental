//! A free-globe pose standing on the ground and looking up at the moon, past 90 degrees of
//! pitch, centres the moon in the frame and still draws the ground and the sky below it.
#![allow(clippy::expect_used, clippy::panic)]

use cgmath::{InnerSpace, Matrix, Matrix3, Quaternion, Vector3};

use crate::{
    celestial::{ecef_to_globe_world, Disc, EcefDirection},
    headless::map::{readback::read_blocking, HeadlessMap},
    projection::globe::EARTH_RADIUS_METERS,
    render::{
        projection::globe_camera_for_view,
        view_state::{GlobePose, NavigationMode},
        RenderPlugin,
    },
    style::Style,
};

const SIZE: u32 = 256;
const FIELD_OF_VIEW_DEGREES: f64 = 90.0;
/// The observer at 39.04855° N, 77.11311° W, who sees the moon at azimuth 92.175° and
/// elevation 37.001° at 2026-10-27T01:42:00Z.
const LATITUDE: f64 = 39.04855;
const LONGITUDE: f64 = -77.11311;
const AZIMUTH: f64 = 92.175;
const ELEVATION: f64 = 37.001;
const EYE_HEIGHT_METERS: f64 = 2.0;
/// A moon 3° across, wide enough to centre to a pixel in this frame.
const MOON_RADIUS_DEGREES: f64 = 1.5;

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

/// The moon's direction seen from the observer, in globe world axes.
fn moon_direction() -> Vector3<f64> {
    let (up, east, north) = local_axes();
    let (azimuth, elevation) = (AZIMUTH.to_radians(), ELEVATION.to_radians());
    (east * azimuth.sin() + north * azimuth.cos()) * elevation.cos() + up * elevation.sin()
}

/// The eye `EYE_HEIGHT_METERS` above sea level at the observer, in globe world metres.
fn standing_eye() -> Vector3<f64> {
    local_axes().0 * (EARTH_RADIUS_METERS + EYE_HEIGHT_METERS)
}

/// The pose with its eye at `eye`, in globe world metres, looking along `forward` level with
/// the observer's horizon across the frame, at a target `distance` metres ahead.
fn pose_at(eye: Vector3<f64>, forward: Vector3<f64>, distance: f64) -> GlobePose {
    let (up, ..) = local_axes();
    let right = forward.cross(up).normalize();
    let camera_up = right.cross(forward);
    // The camera looks along its negative z with y up; the rows take the body's frame into it.
    let rotation = Matrix3::from_cols(right, camera_up, -forward).transpose();
    let orientation = Quaternion::from(rotation);
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

/// The pose standing at the observer and looking along `forward`, its target 1 km ahead.
fn eye_pose(forward: Vector3<f64>) -> GlobePose {
    pose_at(standing_eye(), forward, 1000.0)
}

/// The eye the map draws from, in globe world metres.
fn drawn_eye(map: &HeadlessMap) -> Vector3<f64> {
    let camera = globe_camera_for_view(map.view_state()).expect("camera");
    camera.camera_position() * EARTH_RADIUS_METERS
}

/// Runs a frame, then lets the tiles it asked for arrive; whether the frame drew.
async fn step(map: &mut HeadlessMap) -> bool {
    let drawn = map.run_frame().is_ok();
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    drawn
}

/// The view at the observer looking up at the moon, posed before its first frame and kept
/// while the terrain under its raised target loads.
async fn standing_view() -> HeadlessMap {
    let mut map = standing_view_without_pose().await;
    map.restore_globe_pose(eye_pose(moon_direction()))
        .expect("a pose looking up is placed before the first frame");
    for _ in 0..40 {
        assert!(
            step(&mut map).await,
            "every frame draws as the terrain loads"
        );
    }
    map
}

/// The view at the observer before the camera is posed.
async fn standing_view_without_pose() -> HeadlessMap {
    let style: Style = serde_json::from_str(r##"{"version":8,"center":[-77.11311,39.04855],"zoom":16,"sources":{"dem":{"type":"raster-dem","tiles":["https://dem.example/{z}/{x}/{y}.png"],"encoding":"terrarium"}},"layers":[{"id":"background","type":"background","paint":{"background-color":"#204060"}}],"terrain":{"source":"dem"},"projection":{"type":"vertical-perspective"}}"##).expect("style");
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
    map.set_vertical_field_of_view(cgmath::Deg(FIELD_OF_VIEW_DEGREES));
    map.set_atmosphere(true);
    map
}

/// The ECEF direction of a globe world direction.
fn ecef(globe: Vector3<f64>) -> EcefDirection {
    EcefDirection::new(globe.z, globe.x, globe.y).expect("direction")
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

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 3] {
    let at = ((y * SIZE + x) * 4) as usize;
    [pixels[at], pixels[at + 1], pixels[at + 2]]
}

/// Sokoly's moonlight case: the camera stands 2 m up at the observer, posed through
/// [`GlobePose`] to look at the moon 37° above the horizon, a pitch of 127°.
#[tokio::test]
async fn looking_up_from_the_ground_centres_the_moon_above_the_drawn_horizon() {
    let mut map = standing_view().await;
    let camera = globe_camera_for_view(map.view_state()).expect("camera");
    let eye_height = (camera.camera_position().magnitude() - 1.0) * EARTH_RADIUS_METERS;
    let (up, east, _) = local_axes();
    let centre_view = {
        let far = camera.inverse_view_projection() * cgmath::Vector4::new(0.0, 0.0, 0.5, 1.0);
        (far.truncate() / far.w - camera.camera_position()).normalize()
    };
    let centre_elevation = centre_view.dot(up).asin().to_degrees();

    // A near-full moon at night: the sun stands opposite it.
    map.set_sun_direction(Some(ecef(-moon_direction())));
    let without = frame(&mut map);
    map.set_moon_disc(Some(
        Disc::new(ecef(moon_direction()), MOON_RADIUS_DEGREES).expect("disc"),
    ));
    let with = frame(&mut map);
    let (mut sum, mut count) = ((0.0, 0.0), 0);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (a, b) = (pixel(&with, x, y), pixel(&without, x, y));
            if (0..3).any(|c| a[c].saturating_sub(b[c]) > 40) {
                sum.0 += f64::from(x) + 0.5;
                sum.1 += f64::from(y) + 0.5;
                count += 1;
            }
        }
    }
    let moon = (
        sum.0 / f64::from(count.max(1)),
        sum.1 / f64::from(count.max(1)),
    );
    let half = f64::from(SIZE) / 2.0;
    // The horizon lies 37° below the centre: tan(37°) of the focal length, half the frame.
    let horizon = half + half * ELEVATION.to_radians().tan();
    let night_ground = pixel(&without, SIZE / 2, (horizon + 20.0) as u32);

    // By day the sky and the ground are told apart from the clear colour and each other.
    map.set_moon_disc(None);
    map.set_sun_direction(Some(ecef(up * 0.9 + east * 0.1)));
    let day = frame(&mut map);
    let sky = pixel(&day, SIZE / 2, (horizon - 30.0) as u32);
    let ground = pixel(&day, SIZE / 2, (horizon + 20.0) as u32);
    let report = format!(
        "eye {eye_height:.2} m, frame centre at {centre_elevation:.3}° elevation, moon {count} px \
         at ({:.1}, {:.1}), horizon row {horizon:.1}, sky {sky:?}, ground {ground:?} by day, {night_ground:?} by night",
        moon.0, moon.1,
    );
    assert!((eye_height - EYE_HEIGHT_METERS).abs() < 0.01, "{report}");
    assert!((centre_elevation - ELEVATION).abs() < 0.01, "{report}");
    let pitch = map.view_state().free_globe_pitch().expect("free pose").0;
    assert!(
        (pitch - (90.0 + ELEVATION)).abs() < 0.01,
        "pitch {pitch}°: {report}"
    );
    assert!(count > 10, "the moon is drawn: {report}");
    assert!(
        (moon.0 - half).abs() < 1.0 && (moon.1 - half).abs() < 1.0,
        "the moon lands at the frame centre: {report}"
    );
    assert!(
        sky.iter().any(|&c| c > 40),
        "the atmosphere is drawn above the horizon: {report}"
    );
    assert!(
        ground.iter().any(|&c| c > 20),
        "the ground is drawn below it: {report}"
    );
    assert!(sky != ground, "the sky and the ground differ: {report}");
}

/// The direction `degrees` below the horizon towards the moon's azimuth.
fn towards_moon_below_horizon(degrees: f64) -> Vector3<f64> {
    let (up, east, north) = local_axes();
    let (azimuth, depression) = (AZIMUTH.to_radians(), degrees.to_radians());
    (east * azimuth.sin() + north * azimuth.cos()) * depression.cos() - up * depression.sin()
}

/// A pose looking down from 300 m with its target held 100 m ahead in the air still takes the
/// ground under its target, its eye carried down with it and kept above the ground.
#[tokio::test]
async fn a_pose_looking_down_still_stands_its_target_on_the_ground() {
    let mut map = standing_view_without_pose().await;
    let eye = local_axes().0 * (EARTH_RADIUS_METERS + 300.0);
    let placed = pose_at(eye, towards_moon_below_horizon(30.0), 100.0);
    map.restore_globe_pose(placed).expect("pose");
    for _ in 0..40 {
        assert!(
            step(&mut map).await,
            "every frame draws as the terrain loads"
        );
    }
    let pose = map.view_state().globe_pose().expect("free pose");
    assert!(
        pose.target_elevation_meters.abs() < 0.01,
        "the target stands on the ground: {pose:?}"
    );
    assert_eq!(pose.target, placed.target, "in the same direction");
    // The target came down 250 m, from 250 m to the ground, and the eye with it.
    let altitude = drawn_eye(&map).magnitude() - EARTH_RADIUS_METERS;
    assert!((altitude - 50.0).abs() < 0.5, "the eye at {altitude} m");
}

/// A fly-in from an overview looking down to the eye looking up at the moon, pose by pose,
/// as a host interpolating poses places them: every pose draws with its eye above the ground,
/// and the eye looking up stays where it was placed.
#[tokio::test]
async fn a_fly_in_to_an_eye_looking_up_draws_every_pose() {
    let mut map = standing_view_without_pose().await;
    let (up, ..) = local_axes();
    let start_eye = up * (EARTH_RADIUS_METERS + 5000.0) - towards_moon_below_horizon(0.0) * 3000.0;
    let start_forward = towards_moon_below_horizon(60.0);
    let steps = 12;
    for index in 0..=steps {
        let t = f64::from(index) / f64::from(steps);
        let eye = start_eye * (1.0 - t) + standing_eye() * t;
        let forward = (start_forward * (1.0 - t) + moon_direction() * t).normalize();
        map.restore_globe_pose(pose_at(eye, forward, 1000.0))
            .unwrap_or_else(|error| panic!("pose {index}: {error}"));
        for _ in 0..3 {
            assert!(step(&mut map).await, "pose {index} draws");
        }
        let altitude = drawn_eye(&map).magnitude() - EARTH_RADIUS_METERS;
        assert!(altitude > 0.0, "pose {index}: the eye at {altitude} m");
    }
    let moved = (drawn_eye(&map) - standing_eye()).magnitude();
    assert!(moved < 0.01, "the eye looking up moved {moved} m");
}
