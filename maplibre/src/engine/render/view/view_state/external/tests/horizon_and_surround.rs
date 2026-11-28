use cgmath::{Deg, InnerSpace, Matrix4};

use super::{posed_view_state, view_state, ExternalView, FOVY, MERCATOR};
use crate::{
    coords::LatLon,
    render::view_state::{CameraPose, ViewState},
};

/// The map's own camera posed `pitch` from straight down with `roll`, three kilometres up
/// over ground at 650 metres.
fn own_view_state(pitch: Deg<f64>, roll: Deg<f64>) -> ViewState {
    let mut own = view_state(10.0, LatLon::new(47.0, 11.0));
    own.set_center_elevation(650.0);
    own.set_camera_pose(CameraPose {
        position: LatLon::new(47.3, 11.4),
        altitude_meters: 3000.0,
        bearing: Deg(35.0),
        pitch,
        roll,
    });
    own
}

/// A view state driven by an eye posed as [`own_view_state`], fed through the host's form so
/// the eye path is in effect.
fn eye_view_state(pitch: Deg<f64>, roll: Deg<f64>) -> ViewState {
    let own = own_view_state(pitch, roll);
    let mut eyed = view_state(4.0, LatLon::new(0.0, 0.0));
    eyed.set_center_elevation(650.0);
    eyed.set_max_pitch(Deg(60.0));
    eyed.set_external_view(own.external_view(), &MERCATOR)
        .expect("the eye is accepted");
    eyed
}

#[test]
fn the_horizon_follows_the_eye_rather_than_the_clamped_pose() {
    use cgmath::Point2;

    let level = eye_view_state(Deg(90.0), Deg(0.0));
    let center = Point2::new(level.width() / 2.0, level.height() / 2.0);
    let horizon = level.horizon_line();
    // The sky reaches a few pixels below the true horizon at the screen center, over the
    // void beyond the plane's polar edge.
    let overlap = horizon.sky_distance(center);
    assert!(
        (1.5..6.0).contains(&overlap),
        "a level gaze has the horizon just through the screen center: {horizon:?}"
    );
    assert!(
        (horizon.normal - cgmath::Vector2::unit_y()).magnitude() < 1e-9,
        "the sky is straight up: {horizon:?}"
    );

    // Twenty degrees below level the horizon climbs by the focal length times tan 20.
    let down = eye_view_state(Deg(70.0), Deg(0.0));
    let focal_length = (down.height() / 2.0) / (FOVY.0 / 2.0).tan();
    let expected = focal_length * 20.0_f64.to_radians().tan();
    let climbed = -down.horizon_line().sky_distance(center) + overlap;
    // The two eyes' overlaps over the plane's edge differ by a few parts per million.
    assert!(
        (climbed - expected).abs() < 1e-5 * expected,
        "the horizon is {climbed} pixels above the center, expected {expected}"
    );

    // A rolled eye tilts the line the way the map's own camera would.
    let rolled = eye_view_state(Deg(70.0), Deg(30.0));
    let mut own = view_state(10.0, LatLon::new(47.0, 11.0));
    own.set_camera_pose(CameraPose {
        roll: Deg(30.0),
        ..rolled.camera_pose()
    });
    let own_normal = own.horizon_line().normal;
    assert!(
        (rolled.horizon_line().normal - own_normal).magnitude() < 1e-9,
        "{:?} vs {own_normal:?}",
        rolled.horizon_line()
    );

    // Straight down there is no sky on any screen.
    let nadir = eye_view_state(Deg(0.0), Deg(0.0));
    for corner in [
        Point2::new(0.0, 0.0),
        Point2::new(nadir.width(), nadir.height()),
    ] {
        assert!(nadir.horizon_line().sky_distance(corner) < 0.0);
    }
}

#[test]
fn an_eyes_fog_stays_put_as_the_gaze_moves() {
    // One eye over one anchor, turned about its own right axis: thirty degrees up to a
    // level gaze, twenty degrees further down.
    let base = own_view_state(Deg(60.0), Deg(0.0)).external_view();
    let level = ExternalView {
        view: Matrix4::from_angle_x(Deg(-30.0)) * base.view,
        ..base
    };
    let turned = ExternalView {
        view: Matrix4::from_angle_x(Deg(20.0)) * base.view,
        ..base
    };
    let mut ahead = view_state(4.0, LatLon::new(0.0, 0.0));
    ahead
        .set_external_view(level, &MERCATOR)
        .expect("the level eye is accepted");
    let mut down = view_state(4.0, LatLon::new(0.0, 0.0));
    down.set_external_view(turned, &MERCATOR)
        .expect("the turned eye is accepted");

    // Each eye keeps its own bookkeeping zoom, so compare the distances in metres.
    let in_meters = |view_state: &ViewState| {
        let (near, far) = view_state.fog_depth_range();
        let pixels_per_meter = view_state.pixels_per_meter();
        (near / pixels_per_meter, far / pixels_per_meter)
    };
    let (near, far) = in_meters(&ahead);
    let (near_down, far_down) = in_meters(&down);
    assert!(
        (near - near_down).abs() < 1e-2 * near && (far - far_down).abs() < 1e-2 * far,
        "{near}..{far} vs {near_down}..{far_down}"
    );
    // The fog ends at the geometric horizon of the eye's height above the anchor.
    let height = 3000.0 - level.anchor.altitude_meters;
    let horizon_meters = (2.0_f64 * 6_371_008.8 * height).sqrt();
    assert!(
        (far_down - horizon_meters).abs() < 1e-2 * horizon_meters,
        "far {far_down} m vs horizon {horizon_meters} m at {height} m"
    );
    assert!(near > 0.0 && near < far);
    assert_eq!(ahead.fog_opacity(), 1.0);
    assert_eq!(down.fog_opacity(), 1.0);

    // The map's own camera keeps the GL JS pitch ramp.
    assert_eq!(posed_view_state().fog_opacity(), 0.0);
}

#[test]
fn the_surround_waits_until_the_eyes_zoom_settles() {
    let mut injected = view_state(4.0, LatLon::new(0.0, 0.0));
    injected.set_center_elevation(650.0);
    let resting = posed_view_state().external_view();
    injected
        .set_external_view(resting, &MERCATOR)
        .expect("invertible");
    assert!(
        injected.surround(2.0, &MERCATOR).is_some(),
        "a first eye counts as settled"
    );
    injected
        .set_external_view(resting, &MERCATOR)
        .expect("invertible");
    assert!(
        injected.surround(2.0, &MERCATOR).is_some(),
        "an eye that stayed put is settled"
    );

    // The eye climbed a zoom level within a frame, as on a flight.
    let climbed = view_state(3.0, LatLon::new(0.0, 0.0)).external_view();
    injected
        .set_external_view(climbed, &MERCATOR)
        .expect("invertible");
    assert!(
        injected.surround(2.0, &MERCATOR).is_none(),
        "no surround while the zoom moves"
    );
    injected
        .set_external_view(climbed, &MERCATOR)
        .expect("invertible");
    assert!(
        injected.surround(2.0, &MERCATOR).is_some(),
        "the surround returns once the eye rests"
    );
}
