//! Free poses past 90 degrees of pitch, which the globe camera draws while it looks up at a
//! raised target, and the gestures that would take one where it cannot be drawn.

use cgmath::{Deg, Point2, Rad};

use super::{
    assert_same_matrix, matrix, pitch_and_roll, pose_at, view, NavigationError, NavigationMode,
    VERTICAL,
};
use crate::{
    coords::LatLon,
    render::{projection::globe_camera_for_view, view_state::ViewState},
};

/// A constrained view at zoom 14 over a target raised 3000 m, pitch limit 120 degrees, as
/// GL JS allows up to 180.
fn raised(pitch: f64) -> ViewState {
    let mut state = view(LatLon::new(40.0, 10.0), 14.0, 30.0, 0.0, 0.0);
    state.set_max_pitch(Deg(120.0));
    state.camera_mut().set_pitch(Deg(pitch));
    state.set_center_altitude(3000.0);
    state
}

#[test]
fn a_pose_past_ninety_degrees_that_the_camera_draws_is_free_to_enter_turn_and_restore() {
    // Entering free navigation from a constrained camera looking up at its raised center.
    let mut state = raised(110.0);
    let before = matrix(&state);
    state
        .set_navigation_mode(NavigationMode::FreeGlobe, &VERTICAL)
        .expect("free navigation from 110 degrees");
    assert_same_matrix(matrix(&state), before, "entered at 110");
    // Turning across 90 degrees, and storing and restoring where the turn ends.
    let mut state = raised(80.0);
    state
        .set_navigation_mode(NavigationMode::FreeGlobe, &VERTICAL)
        .expect("free navigation");
    let start = state.globe_pose().expect("pose");
    state
        .orbit_globe_pose(start, Rad(0.0), Rad(30_f64.to_radians()))
        .expect("orbit across 90 degrees");
    assert!((pitch_and_roll(&state).0 - 110.0).abs() < 1e-9);
    let turned = state.globe_pose().expect("pose");
    let at = matrix(&state);
    let mut restored = raised(0.0);
    restored
        .restore_globe_pose(turned, &VERTICAL)
        .expect("a turned pose restores");
    assert_same_matrix(matrix(&restored), at, "restored at 110");
}

#[test]
fn a_pose_whose_eye_would_be_inside_the_body_is_refused_and_the_view_kept() {
    let center = LatLon::new(40.0, 10.0);
    let mut state = view(center, 5.0, 0.0, 30.0, 0.0);
    state
        .set_navigation_mode(NavigationMode::FreeGlobe, &VERTICAL)
        .expect("free navigation");
    let pose = state.globe_pose().expect("pose");
    let kept = matrix(&state);
    // Looking up at a target on the ground from 80 km away puts the eye deep underground.
    let under = pose_at(center, 0.0, 120.0, 0.0);
    assert!(matches!(
        state.restore_globe_pose(under, &VERTICAL),
        Err(NavigationError::InvalidPose { .. })
    ));
    assert_eq!(state.globe_pose(), Some(pose));
    assert_eq!(matrix(&state), kept);
    // A turn that would take the eye there is refused the same way.
    state.set_max_pitch(Deg(170.0));
    assert!(matches!(
        state.orbit_globe_pose(pose, Rad(0.0), Rad(130_f64.to_radians())),
        Err(NavigationError::InvalidPose { .. })
    ));
    assert_eq!(matrix(&state), kept);
}

/// The free camera's flat state, its style zoom, center and pitch, is what its pose shows.
fn assert_flat_camera_follows_the_pose(state: &ViewState, case: &str) {
    let shown = state.pose_view().expect("free camera");
    assert!(
        (state.zoom().value() - shown.style_zoom).abs() < 1e-9,
        "{case}: zoom {} for a pose shown at {}",
        state.zoom().value(),
        shown.style_zoom
    );
    let position = state.camera().position();
    let world_size = crate::coords::TILE_SIZE * 2_f64.powf(state.zoom().value());
    let center =
        crate::render::projection::mercator_world_to_lat_lon(position.x, position.y, world_size);
    assert!(
        (center.latitude - shown.center.latitude).abs() < 1e-9
            && (center.longitude - shown.center.longitude).abs() < 1e-9,
        "{case}: centered at {center:?} for a pose over {:?}",
        shown.center
    );
    let pitch = state.camera().get_pitch().0.to_degrees();
    assert!(
        (pitch - shown.pitch_degrees).abs() < 1e-9,
        "{case}: pitch {pitch} for a pose at {}",
        shown.pitch_degrees
    );
}

#[test]
fn a_restored_or_turned_pose_carries_the_flat_camera_with_it() {
    let mut state = raised(80.0);
    state
        .set_navigation_mode(NavigationMode::FreeGlobe, &VERTICAL)
        .expect("free navigation");
    let start = state.globe_pose().expect("pose");
    state
        .orbit_globe_pose(start, Rad(0.0), Rad(30_f64.to_radians()))
        .expect("orbit");
    assert_flat_camera_follows_the_pose(&state, "turned");
    let turned = state.globe_pose().expect("pose");
    // Restored into a view far from the pose, at another zoom and place.
    let mut restored = view(LatLon::new(-20.0, 120.0), 3.0, 0.0, 0.0, 0.0);
    restored.set_max_pitch(Deg(120.0));
    restored
        .restore_globe_pose(turned, &VERTICAL)
        .expect("restore");
    assert_flat_camera_follows_the_pose(&restored, "restored");
}

#[test]
fn zooming_out_a_pose_past_ninety_degrees_stops_where_the_camera_can_still_be_drawn() {
    let mut state = raised(110.0);
    state
        .set_navigation_mode(NavigationMode::FreeGlobe, &VERTICAL)
        .expect("free navigation from 110 degrees");
    let pixel = Point2::new(state.width() / 2.0, state.height() / 2.0);
    let mut stopped = false;
    let mut partial = false;
    for _ in 0..40 {
        let distance = state.globe_pose().expect("pose").distance_meters;
        let moved = state.zoom_free_globe(pixel, -0.5);
        globe_camera_for_view(&state).expect("every zoom leaves a drawable camera");
        let ratio = state.globe_pose().expect("pose").distance_meters / distance;
        if !moved {
            assert_eq!(ratio, 1.0);
            stopped = true;
            break;
        }
        partial |= ratio < 2_f64.sqrt() - 1e-9;
    }
    assert!(
        stopped,
        "zooming out stops before the eye goes under the ground"
    );
    assert!(partial, "the last steps go as far as a camera can be drawn");
    let pose = state.globe_pose().expect("pose");
    assert!(pitch_and_roll(&state).0 > 90.0);
    // At the stop the camera is still free to zoom back in and to be dragged.
    assert!(state.zoom_free_globe(pixel, 0.5));
    assert!(state.globe_pose().expect("pose").distance_meters < pose.distance_meters);
    // A drag turns eye and target together, so it keeps the camera drawable however far it goes.
    for step in 0..40 {
        let to = pixel + cgmath::Vector2::new(60.0, if step < 20 { 40.0 } else { -40.0 });
        assert!(state.drag_free_globe(pixel, to), "drag {step}");
        globe_camera_for_view(&state).expect("a drawable camera after every drag");
    }
}
