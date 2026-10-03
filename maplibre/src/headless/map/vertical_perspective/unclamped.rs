//! With terrain present, a center taken off the ground keeps its altitude through frames and
//! gestures, while one on the ground follows the terrain.

use super::*;

#[tokio::test]
async fn an_unclamped_center_keeps_its_altitude_over_terrain_through_frames_and_gestures() {
    let scene = Scene {
        meters: 500.0,
        relief: 0.0,
        exaggeration: 1.0,
        pitch: 60.0,
        zoom: 11.67,
    };
    let mut level = LevelMap::new(scene).await;
    // On the ground the center follows the terrain.
    level.settle(Some(500.0)).await;
    assert!(level.map.view_state().center_clamped_to_ground());
    // Pinned off the ground, as a fixture unclamping the center does, it stays put.
    level.map.pin_center_elevation(3000.0);
    assert!(!level.map.view_state().center_clamped_to_ground());
    for _ in 0..6 {
        level.map.run_frame().expect("frame");
    }
    assert_eq!(level.map.view_state().center_elevation(), 3000.0);
    // A gesture ending over it recomputes nothing for a center off the ground.
    let state = &mut level.map.map_context.view_state;
    let (zoom, position) = (state.zoom().value(), state.camera().position());
    crate::terrain::interaction::begin_gesture(state);
    crate::terrain::interaction::finish_gesture(
        &level.map.map_context.style,
        &mut level.map.map_context.view_state,
        &level.map.map_context.world,
    );
    let state = level.map.view_state();
    assert_eq!(state.center_elevation(), 3000.0);
    assert_eq!(state.zoom().value(), zoom);
    assert_eq!(state.camera().position(), position);
    // Back on the ground it follows the terrain again.
    level
        .map
        .map_context
        .view_state
        .set_center_clamped_to_ground(true);
    for _ in 0..6 {
        level.map.run_frame().expect("frame");
    }
    assert_eq!(level.map.view_state().center_elevation(), 500.0);
}

#[tokio::test]
async fn a_restored_free_pose_keeps_its_height_off_the_ground_and_takes_the_ground_on_it() {
    use crate::render::view_state::NavigationMode;
    let scene = Scene {
        meters: 500.0,
        relief: 0.0,
        exaggeration: 1.0,
        pitch: 60.0,
        zoom: 11.67,
    };
    let mut level = LevelMap::new(scene).await;
    level.settle(Some(500.0)).await;
    level
        .map
        .set_navigation_mode(NavigationMode::FreeGlobe)
        .expect("free navigation");
    let mut pose = level.map.view_state().globe_pose().expect("pose");
    pose.target_elevation_meters = 3000.0;
    // Off the ground the restored height holds.
    level
        .map
        .map_context
        .view_state
        .set_center_clamped_to_ground(false);
    level.map.restore_globe_pose(pose).expect("restore");
    for _ in 0..4 {
        level.map.run_frame().expect("frame");
    }
    let held = level.map.view_state().globe_pose().expect("pose");
    assert_eq!(held.target_elevation_meters, 3000.0);
    assert_eq!(held.target, pose.target);
    // On the ground the terrain under the target decides it.
    level
        .map
        .map_context
        .view_state
        .set_center_clamped_to_ground(true);
    for _ in 0..4 {
        level.map.run_frame().expect("frame");
    }
    let held = level.map.view_state().globe_pose().expect("pose");
    assert_eq!(held.target_elevation_meters, 500.0);
    assert_eq!(held.target, pose.target);
}
