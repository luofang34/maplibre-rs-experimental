#![allow(clippy::expect_used, clippy::panic)]

use cgmath::{Deg, Point2, Rad, Vector4};

use super::CameraPose;
use crate::{
    coords::{LatLon, WorldCoords, Zoom},
    render::view_state::ViewState,
    window::PhysicalSize,
};

const FOVY: Rad<f64> = Rad(0.6435011087932844);

fn view_state() -> ViewState {
    let zoom = Zoom::new(10.0);
    let mut view_state = ViewState::new(
        PhysicalSize::new(800, 600).expect("non-zero size"),
        WorldCoords::from_lat_lon(LatLon::new(47.0, 11.0), zoom),
        zoom,
        Deg(0.0),
        FOVY,
    );
    view_state.set_max_pitch(Deg(180.0));
    view_state
}

fn pose() -> CameraPose {
    CameraPose {
        position: LatLon::new(47.3, 11.4),
        altitude_meters: 3000.0,
        bearing: Deg(35.0),
        pitch: Deg(55.0),
        roll: Deg(12.0),
    }
}

#[test]
fn a_camera_pose_survives_the_round_trip_through_center_and_zoom() {
    let mut view_state = view_state();
    let pose = pose();

    view_state.set_camera_pose(pose);
    let back = view_state.camera_pose();

    assert!(
        (back.position.latitude - pose.position.latitude).abs() < 1e-9
            && (back.position.longitude - pose.position.longitude).abs() < 1e-9,
        "{back:?}"
    );
    assert!(
        (back.altitude_meters - pose.altitude_meters).abs() < 1e-6,
        "{back:?}"
    );
    assert!((back.bearing.0 - pose.bearing.0).abs() < 1e-9, "{back:?}");
    assert!((back.pitch.0 - pose.pitch.0).abs() < 1e-9, "{back:?}");
    assert!((back.roll.0 - pose.roll.0).abs() < 1e-9, "{back:?}");
}

#[test]
fn the_center_lies_where_the_view_ray_meets_the_ground() {
    let mut view_state = view_state();
    let pose = pose();

    view_state.set_camera_pose(pose);

    let pixels_per_meter = view_state.pixels_per_meter();
    let camera = WorldCoords::from_lat_lon(pose.position, view_state.zoom());
    let center = view_state.camera().position();
    let ground_distance =
        ((center.x - camera.x).powi(2) + (center.y - camera.y).powi(2)).sqrt() / pixels_per_meter;
    let pitch = Rad::from(pose.pitch).0;
    assert!(
        (ground_distance - pose.altitude_meters * pitch.tan()).abs() < 0.01,
        "ground distance {ground_distance}"
    );
    let view_distance = view_state.camera_to_center_distance() / pixels_per_meter;
    assert!(
        (view_distance - pose.altitude_meters / pitch.cos()).abs() < 0.01,
        "view distance {view_distance}"
    );
    let north_of_camera = camera.y - center.y;
    let east_of_camera = center.x - camera.x;
    assert!(
        north_of_camera > 0.0 && east_of_camera > 0.0,
        "a bearing of 35 degrees looks north-east"
    );
}

#[test]
fn a_pitch_beyond_ninety_degrees_looks_up_from_below_the_center() {
    let mut view_state = view_state();
    view_state.set_center_elevation(500.0);
    view_state.camera_mut().set_pitch(Deg(120.0));

    assert_eq!(view_state.camera().get_pitch(), Rad::from(Deg(120.0)));
    let (near, far) = view_state.depth_range(Point2::new(0.0, 0.0));
    assert!(
        near > 0.0 && far.is_finite() && far > near,
        "near {near} far {far}"
    );
    let expected_altitude = Rad::from(Deg(120.0_f64)).0.cos()
        * view_state.camera_to_center_distance()
        / view_state.pixels_per_meter()
        + 500.0;
    let eye = view_state.eye_position();
    assert!(
        (eye.z - expected_altitude).abs() < 1e-6 && eye.z < 500.0,
        "eye {eye:?}, expected altitude {expected_altitude}"
    );
}

fn screen_position(view_state: &ViewState, offset: (f64, f64)) -> (f64, f64) {
    let center = view_state.camera().position();
    let clip = view_state.view_projection().0
        * Vector4::new(center.x + offset.0, center.y + offset.1, 0.0, 1.0);
    (clip.x / clip.w, clip.y / clip.w)
}

#[test]
fn a_roll_of_ninety_degrees_turns_east_onto_the_vertical_axis() {
    let mut view_state = view_state();
    let (x, y) = screen_position(&view_state, (100.0, 0.0));
    assert!(
        x > 0.01 && y.abs() < 1e-9,
        "east lies to the right without roll: {x} {y}"
    );

    view_state.camera_mut().set_roll(Deg(90.0));

    let (x, y) = screen_position(&view_state, (100.0, 0.0));
    assert!(
        x.abs() < 1e-9 && y.abs() > 0.01,
        "east lies on the vertical axis: {x} {y}"
    );
}
