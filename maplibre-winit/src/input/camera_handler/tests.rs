#![allow(clippy::expect_used, clippy::panic)]

use cgmath::{Deg, Vector2};
use maplibre::{
    coords::{LatLon, WorldCoords, Zoom},
    projection::ProjectionType,
    render::{
        projection::globe_camera_for_view,
        view_state::{NavigationMode, ViewState},
    },
    window::PhysicalSize,
};
use winit::event::{ElementState, MouseButton};

use super::CameraHandler;

/// A view at 60 degrees north, 40 degrees of pitch, in free navigation when `free`.
fn view(free: bool) -> ViewState {
    let zoom = Zoom::new(5.0);
    let mut view = ViewState::new(
        PhysicalSize::new(800, 600).expect("viewport"),
        WorldCoords::from_lat_lon(LatLon::new(60.0, 20.0), zoom),
        zoom,
        Deg(40.0),
        Deg(36.87),
    );
    view.set_max_pitch(Deg(85.0));
    view.camera_mut().set_pitch(Deg(40.0));
    if free {
        view.set_navigation_mode(
            NavigationMode::FreeGlobe,
            &ProjectionType::VerticalPerspective,
        )
        .expect("free navigation");
    }
    view
}

/// Drags with `button` from `from` through `through`, turning the view after each move, as
/// the input loop does; 100 sensitivity is 0.1 degrees a pixel.
fn drag(view: &mut ViewState, button: MouseButton, from: Vector2<f64>, through: &[Vector2<f64>]) {
    let mut handler = CameraHandler::new(100.0);
    handler.process_window_position(&from, false);
    handler.process_mouse_key_press(&button, &ElementState::Pressed);
    for position in through {
        handler.process_window_position(position, false);
        handler.turn(view);
    }
    handler.process_mouse_key_press(&button, &ElementState::Released);
    assert_eq!(
        handler.start_pose, None,
        "a release forgets the drag's start"
    );
}

fn angles(view: &ViewState) -> (f64, f64) {
    let camera = globe_camera_for_view(view).expect("camera");
    (camera.bearing_degrees(), camera.pitch_degrees())
}

#[test]
fn a_right_drag_turns_a_free_camera_about_its_target() {
    let mut free = view(true);
    let before = free.globe_pose().expect("pose");
    let (bearing, pitch) = angles(&free);
    // Left by 200 pixels and up by 100: 20 degrees of bearing and 10 of pitch.
    drag(
        &mut free,
        MouseButton::Right,
        Vector2::new(400.0, 300.0),
        &[Vector2::new(300.0, 250.0), Vector2::new(200.0, 200.0)],
    );
    let after = free.globe_pose().expect("pose");
    assert_eq!(after.target, before.target, "the target stays");
    assert_eq!(after.distance_meters, before.distance_meters);
    let (turned_bearing, turned_pitch) = angles(&free);
    let turn = (turned_bearing - bearing + 540.0).rem_euclid(360.0) - 180.0;
    assert!((turn - 20.0).abs() < 1e-6, "bearing turned by {turn}");
    assert!(
        (turned_pitch - pitch - 10.0).abs() < 1e-6,
        "pitch {pitch} -> {turned_pitch}"
    );
    // One move to the end turns as far as the same drag in steps: the turn is from the start.
    let mut once = view(true);
    drag(
        &mut once,
        MouseButton::Right,
        Vector2::new(400.0, 300.0),
        &[Vector2::new(200.0, 200.0)],
    );
    let (once_bearing, once_pitch) = angles(&once);
    assert!(
        (once_bearing - turned_bearing).abs() < 1e-9 && (once_pitch - turned_pitch).abs() < 1e-9
    );
}

#[test]
fn a_middle_drag_turns_only_the_bearing_and_a_constrained_camera_turns_as_before() {
    let mut free = view(true);
    let (bearing, pitch) = angles(&free);
    drag(
        &mut free,
        MouseButton::Middle,
        Vector2::new(400.0, 300.0),
        &[Vector2::new(400.0, 150.0)],
    );
    let (turned_bearing, turned_pitch) = angles(&free);
    let turn = (turned_bearing - bearing + 540.0).rem_euclid(360.0) - 180.0;
    assert!((turn - 15.0).abs() < 1e-6, "bearing turned by {turn}");
    assert!((turned_pitch - pitch).abs() < 1e-6);
    let mut constrained = view(false);
    drag(
        &mut constrained,
        MouseButton::Right,
        Vector2::new(400.0, 300.0),
        &[Vector2::new(200.0, 200.0)],
    );
    assert!((constrained.camera().get_bearing().0.to_degrees() - 20.0).abs() < 1e-9);
    assert!((constrained.camera().get_pitch().0.to_degrees() - 50.0).abs() < 1e-9);
}
