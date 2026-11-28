#![allow(clippy::expect_used, clippy::panic)]

use cgmath::Deg;

use super::super::tests::{assert_same_matrix, matrix, view, VERTICAL};
use crate::{
    coords::LatLon,
    render::view_state::{CameraOptions, NavigationMode},
};

/// The ocean floor a long jump starts over.
const OCEAN_FLOOR: f64 = -5468.0;

fn free(center: LatLon) -> crate::render::view_state::ViewState {
    let mut state = view(center, 9.0, 20.0, 50.0, 0.0);
    state
        .set_navigation_mode(NavigationMode::FreeGlobe, &VERTICAL)
        .expect("free navigation");
    state
}

#[test]
fn a_jump_without_a_center_keeps_the_free_camera_where_it_is() {
    for latitude in [-60.0, 0.0, 45.0] {
        let mut state = free(LatLon::new(latitude, 30.0));
        state.set_center_elevation(1200.0);
        let before = matrix(&state);
        state.jump_to(CameraOptions::default()).expect("jump");
        assert_same_matrix(matrix(&state), before, &format!("{latitude}"));
    }
}

#[test]
fn a_free_jump_shows_what_the_constrained_camera_shows() {
    let target = LatLon::new(52.97, 36.06);
    let mut constrained = view(target, 13.0, -18.0, 55.0, 0.0);
    constrained.set_center_elevation(0.0);
    let mut state = free(LatLon::new(-30.0, -20.0));
    state
        .jump_to(CameraOptions {
            center: Some(target),
            zoom: Some(13.0),
            bearing: Some(Deg(-18.0)),
            pitch: Some(Deg(55.0)),
            roll: Some(Deg(0.0)),
            elevation: None,
        })
        .expect("jump");
    assert_eq!(state.navigation_mode(), NavigationMode::FreeGlobe);
    assert_same_matrix(matrix(&state), matrix(&constrained), "Oryol");
}

#[test]
fn a_jump_leaves_the_ground_of_the_old_center_behind() {
    for mode in [NavigationMode::Constrained, NavigationMode::FreeGlobe] {
        let mut state = view(LatLon::new(-30.0, -20.0), 9.0, 0.0, 50.0, 0.0);
        state.set_navigation_mode(mode, &VERTICAL).expect("mode");
        state.set_center_over_terrain(true);
        state.set_center_elevation(OCEAN_FLOOR);
        state
            .jump_to(CameraOptions {
                center: Some(LatLon::new(52.97, 36.06)),
                zoom: Some(15.0),
                ..Default::default()
            })
            .expect("jump");
        assert_eq!(state.center_elevation(), 0.0, "{mode:?}");
        if let Some(pose) = state.globe_pose() {
            assert_eq!(pose.target_elevation_meters, 0.0);
        }
        // Turning the camera in place keeps the ground it stands on.
        state.set_center_elevation(180.0);
        state
            .jump_to(CameraOptions {
                bearing: Some(Deg(40.0)),
                ..Default::default()
            })
            .expect("turn");
        assert_eq!(state.center_elevation(), 180.0, "{mode:?}");
    }
}

#[test]
fn a_jump_rests_the_center_at_the_elevation_it_gives() {
    for mode in [NavigationMode::Constrained, NavigationMode::FreeGlobe] {
        let mut state = view(LatLon::new(-30.0, -20.0), 9.0, 0.0, 50.0, 0.0);
        state.set_navigation_mode(mode, &VERTICAL).expect("mode");
        state.set_center_over_terrain(true);
        state.set_center_elevation(OCEAN_FLOOR);
        state
            .jump_to(CameraOptions {
                center: Some(LatLon::new(52.97, 36.06)),
                elevation: Some(180.0),
                ..Default::default()
            })
            .expect("jump");
        assert_eq!(state.center_elevation(), 180.0, "{mode:?}");
    }
}

#[test]
fn off_the_ground_a_jump_keeps_the_center_altitude() {
    let mut state = free(LatLon::new(10.0, 10.0));
    state.set_center_clamped_to_ground(false);
    state.set_center_altitude(900.0);
    state.set_center_elevation(900.0);
    state
        .jump_to(CameraOptions {
            center: Some(LatLon::new(20.0, 20.0)),
            ..Default::default()
        })
        .expect("jump");
    assert_eq!(state.center_elevation(), 900.0);
}
