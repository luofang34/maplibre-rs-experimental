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

#[tokio::test]
async fn a_free_camera_over_terrain_leaves_the_center_altitude_for_when_it_leaves_the_ground() {
    use cgmath::Rad;

    use crate::render::view_state::NavigationMode;
    let scene = Scene {
        meters: 500.0,
        relief: 0.0,
        exaggeration: 1.0,
        pitch: 60.0,
        zoom: 11.67,
    };
    let mut level = LevelMap::new(scene).await;
    // The altitude the center rests at off the ground, as a style's centerAltitude sets it.
    level.map.map_context.view_state.set_center_altitude(3000.0);
    level.settle(Some(500.0)).await;
    assert_eq!(level.map.view_state().center_altitude(), 3000.0);
    level
        .map
        .set_navigation_mode(NavigationMode::FreeGlobe)
        .expect("free navigation");
    let state = &mut level.map.map_context.view_state;
    let center = Point2::new(f64::from(WIDTH) / 2.0, f64::from(HEIGHT) / 2.0);
    assert!(state.drag_free_globe(center - cgmath::Vector2::new(0.0, 30.0), center));
    let pose = state.globe_pose().expect("pose");
    state
        .orbit_globe_pose(pose, Rad(0.1), Rad(0.05))
        .expect("orbit");
    let turned = state.globe_pose().expect("pose");
    level.map.restore_globe_pose(turned).expect("restore");
    for _ in 0..4 {
        level.map.run_frame().expect("frame");
    }
    let state = level.map.view_state();
    assert_eq!(
        state.center_elevation(),
        500.0,
        "the ground holds the center"
    );
    assert_eq!(
        state.center_altitude(),
        3000.0,
        "the altitude waits off the ground"
    );
    level
        .map
        .map_context
        .view_state
        .set_center_clamped_to_ground(false);
    level.map.run_frame().expect("frame");
    assert_eq!(level.map.view_state().center_elevation(), 3000.0);
}

#[tokio::test]
async fn a_pose_free_follow_after_an_underground_target_stands_the_center_on_its_ground() {
    use cgmath::Deg;

    use crate::{
        coords::{LatLon, WorldCoords, Zoom, TILE_SIZE},
        projection::ProjectionType,
        render::view_state::{NavigationMode, ViewState},
        window::PhysicalSize,
    };
    let dem = SyntheticDem {
        meters: 210.0,
        relief: 150.0,
    };
    let scene = Scene {
        meters: dem.meters,
        relief: dem.relief,
        exaggeration: 1.0,
        pitch: 60.0,
        zoom: 11.67,
    };
    let mut level = LevelMap::new(scene).await;
    level.settle(None).await;
    level
        .map
        .set_navigation_mode(NavigationMode::FreeGlobe)
        .expect("free navigation");
    // A photograph's pose orbits a target along its line of sight, far under the ground,
    // placed on the view alone, which leaves the ground to the next frame.
    let mut pose = level.map.view_state().globe_pose().expect("pose");
    pose.target_elevation_meters = -1784.0;
    level
        .map
        .map_context
        .view_state
        .restore_globe_pose(pose, &ProjectionType::VerticalPerspective)
        .expect("restore");
    // A camera without a pose, as a host seeds one: a fresh view elsewhere, entering free
    // navigation at the elevation the map's center has now.
    let view = &mut level.map.map_context.view_state;
    let zoom = Zoom::new(13.0);
    let centre = WorldCoords::from_lat_lon(LatLon::new(27.79, 88.08), zoom);
    let size = PhysicalSize::new(view.width() as u32, view.height() as u32).expect("size");
    let mut seed = ViewState::new(size, centre, zoom, Deg(0.0), view.field_of_view());
    seed.set_body(view.body());
    seed.set_center_elevation(view.center_elevation());
    seed.camera_mut().set_pitch(Deg(50.0));
    seed.set_navigation_mode(
        NavigationMode::FreeGlobe,
        &ProjectionType::VerticalPerspective,
    )
    .expect("seed enters free navigation");
    let seeded = seed.globe_pose().expect("seed pose");
    assert_eq!(seeded.target_elevation_meters, -1784.0);
    view.restore_globe_pose(seeded, &ProjectionType::VerticalPerspective)
        .expect("follow");
    assert!(level.map.view_state().center_held_by_terrain());
    for _ in 0..8 {
        level.map.run_frame().expect("frame");
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
    }
    let state = level.map.view_state();
    let world_size = TILE_SIZE * 2_f64.powf(state.zoom().value());
    let center = state.camera().position();
    let ground = dem.height(center.x / world_size, center.y / world_size);
    assert!(
        (state.center_elevation() - ground).abs() < 1.0,
        "the center rests at {} m over ground at {ground} m",
        state.center_elevation()
    );
    assert!(
        (ground - dem.meters).abs() > 10.0,
        "the followed center stands on ground of its own, not the scene's mean height"
    );
}

/// A host restoring a pose through the map reads back a center on the ground under the pose's
/// target before any frame runs, so a camera it seeds from that center starts on the ground,
/// not at the pose's target far under it.
#[tokio::test]
async fn a_restored_pose_stands_its_center_on_the_ground_before_any_frame() {
    use crate::coords::TILE_SIZE;
    let dem = SyntheticDem {
        meters: 210.0,
        relief: 150.0,
    };
    let scene = Scene {
        meters: dem.meters,
        relief: dem.relief,
        exaggeration: 1.0,
        pitch: 60.0,
        zoom: 11.67,
    };
    let mut level = LevelMap::new(scene).await;
    level.settle(None).await;
    level
        .map
        .set_navigation_mode(crate::render::view_state::NavigationMode::FreeGlobe)
        .expect("free navigation");
    let mut pose = level.map.view_state().globe_pose().expect("pose");
    pose.target_elevation_meters = -879.0;
    level.map.restore_globe_pose(pose).expect("restore");
    let state = level.map.view_state();
    assert!(state.center_held_by_terrain());
    let world_size = TILE_SIZE * 2_f64.powf(state.zoom().value());
    let center = state.camera().position();
    let ground = dem.height(center.x / world_size, center.y / world_size);
    let read_back = state.globe_pose().expect("pose").target_elevation_meters;
    // The DEM tile at the integer zoom, as GL JS lifts its center, stands within a few
    // metres of the exact ground between its pixels.
    assert!(
        (read_back - ground).abs() < 5.0,
        "the restored center reads back at {read_back} m over ground at {ground} m"
    );
    assert!(
        (ground - dem.meters).abs() > 10.0,
        "the center stands on ground of its own, not the scene's mean height"
    );
    level.map.run_frame().expect("frame");
    let framed = level.map.view_state().center_elevation();
    assert!(
        (framed - read_back).abs() < 1e-6,
        "the frame stands the center where the restore did: {framed} m against {read_back} m"
    );
}
