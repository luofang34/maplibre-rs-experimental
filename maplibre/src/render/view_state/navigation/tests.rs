#![allow(clippy::expect_used, clippy::panic)]

use cgmath::{Deg, InnerSpace, Point2, Vector4};

use super::{GlobePose, NavigationError, NavigationLimit, NavigationMode};
use crate::{
    coords::{LatLon, WorldCoords, Zoom},
    projection::{globe::scale::MERCATOR_LATITUDE_LIMIT, ProjectionType},
    render::{projection::globe_camera_for_view, view_state::ViewState},
    window::PhysicalSize,
};

const VERTICAL: ProjectionType = ProjectionType::VerticalPerspective;

fn view(center: LatLon, zoom: f64, bearing: f64, pitch: f64, roll: f64) -> ViewState {
    let zoom = Zoom::new(zoom);
    let mut view = ViewState::new(
        PhysicalSize::new(1200, 900).expect("size"),
        WorldCoords::from_lat_lon(center, zoom),
        zoom,
        Deg(0.0),
        Deg(36.869_897_645_844_02),
    );
    view.set_max_pitch(Deg(85.0));
    view.camera_mut().set_pitch(Deg(pitch));
    view.camera_mut().set_bearing(Deg(bearing));
    view.camera_mut().set_roll(Deg(roll));
    view.set_globe_orbits_center(true);
    view
}

fn matrix(view: &ViewState) -> [f64; 16] {
    let columns: [[f64; 4]; 4] = globe_camera_for_view(view)
        .expect("camera")
        .view_projection()
        .into();
    let mut flat = [0.0; 16];
    for (index, column) in columns.iter().enumerate() {
        flat[index * 4..index * 4 + 4].copy_from_slice(column);
    }
    flat
}

fn assert_same_matrix(left: [f64; 16], right: [f64; 16], case: &str) {
    let scale = left
        .iter()
        .fold(0.0_f64, |most, value| most.max(value.abs()));
    for (a, b) in left.iter().zip(&right) {
        assert!(
            (a - b).abs() <= 1e-9 * scale,
            "{case}: {left:?} != {right:?}"
        );
    }
}

#[test]
fn free_navigation_starts_where_the_camera_is_and_its_pose_round_trips() {
    for latitude in [-85.0, 0.0, 45.0, 85.0] {
        // Straight down, roll and bearing turn about one axis and fold together.
        for (bearing, pitch, roll) in [
            (0.0, 0.0, 0.0),
            (137.0, 0.0, 20.0),
            (300.0, 0.0, -60.0),
            (137.0, 45.0, 20.0),
            (300.0, 80.0, -15.0),
        ] {
            let case = format!("{latitude}, {bearing}, {pitch}, {roll}");
            let mut state = view(LatLon::new(latitude, 30.0), 5.0, bearing, pitch, roll);
            state.set_center_elevation(1200.0);
            let before = matrix(&state);
            let zoom = state.zoom().value();
            state
                .set_navigation_mode(NavigationMode::FreeGlobe, &VERTICAL)
                .expect("free navigation");
            assert_same_matrix(matrix(&state), before, &case);
            // Inside the Mercator world the style zoom is the north-locked view's.
            assert!((state.zoom().value() - zoom).abs() < 1e-9, "{case}");
            let pose = state.globe_pose().expect("pose");
            let stored = serde_json::to_string(&pose).expect("serialize");
            let mut restored = view(LatLon::new(0.0, 0.0), 1.0, 0.0, 0.0, 0.0);
            restored
                .set_globe_pose(serde_json::from_str::<GlobePose>(&stored).expect("parse"))
                .expect("pose");
            assert_same_matrix(matrix(&restored), before, &format!("{case}, restored"));
            assert_eq!(restored.center_elevation(), 1200.0);
        }
    }
}

#[test]
fn free_navigation_is_refused_or_ended_where_the_projection_turns_flat() {
    let expression: ProjectionType = serde_json::from_str(
        r#"["interpolate",["linear"],["zoom"],10,"vertical-perspective",12,"mercator"]"#,
    )
    .expect("expression");
    for projection in [ProjectionType::Globe, ProjectionType::Mercator, expression] {
        let mut state = view(LatLon::new(40.0, 0.0), 5.0, 0.0, 30.0, 0.0);
        assert!(matches!(
            state.set_navigation_mode(NavigationMode::FreeGlobe, &projection),
            Err(NavigationError::ProjectionNotSupported { .. })
        ));
        assert_eq!(state.navigation_mode(), NavigationMode::NorthLocked);
    }
    let mut state = view(LatLon::new(40.0, 0.0), 5.0, 0.0, 30.0, 0.0);
    state
        .set_navigation_mode(NavigationMode::FreeGlobe, &VERTICAL)
        .expect("free navigation");
    state.enforce_navigation(&VERTICAL);
    assert_eq!(state.navigation_mode(), NavigationMode::FreeGlobe);
    state.enforce_navigation(&ProjectionType::Globe);
    assert_eq!(state.navigation_mode(), NavigationMode::NorthLocked);
    assert_eq!(
        state.navigation_limit(),
        Some(NavigationLimit::ProjectionChanged)
    );
    state
        .set_navigation_mode(NavigationMode::NorthLocked, &ProjectionType::Globe)
        .expect("north-locked");
    assert_eq!(state.navigation_limit(), None, "the host has chosen again");
}

/// The eye and the directions it looks and holds up, in the body's frame.
fn eye_frame(state: &ViewState) -> [cgmath::Vector3<f64>; 3] {
    let camera = globe_camera_for_view(state).expect("camera");
    let inverse = cgmath::SquareMatrix::invert(&camera.view()).expect("view inverse");
    let axis = |v: Vector4<f64>| (inverse * v).truncate().normalize();
    [
        camera.camera_position(),
        axis(Vector4::new(0.0, 0.0, -1.0, 0.0)),
        axis(Vector4::new(0.0, 1.0, 0.0, 0.0)),
    ]
}

#[test]
fn dragging_north_carries_the_camera_over_the_pole_without_a_jump() {
    let mut state = view(LatLon::new(80.0, 10.0), 4.0, 0.0, 50.0, 0.0);
    state
        .set_navigation_mode(NavigationMode::FreeGlobe, &VERTICAL)
        .expect("free navigation");
    let center = Point2::new(600.0, 450.0);
    let (mut previous, mut zoom) = (eye_frame(&state), state.zoom().value());
    let (mut highest, mut crossed) = (0.0_f64, false);
    // Each drag turns the camera by the same few pixels' worth; a jump would be many times that.
    let mut stride: Option<f64> = None;
    for step in 0..3000 {
        assert!(state.drag_free_globe(center - cgmath::Vector2::new(0.0, 12.0), center));
        let frame = eye_frame(&state);
        let latitude = state.pose_view().expect("pose").center.latitude;
        assert!(
            matrix(&state).iter().all(|value| value.is_finite()),
            "step {step}"
        );
        let moved = (0..3)
            .map(|i| (frame[i] - previous[i]).magnitude())
            .fold(0.0, f64::max);
        let first = *stride.get_or_insert(moved);
        assert!(
            moved > 0.0 && moved < 3.0 * first,
            "step {step} at {latitude}: the camera moves {moved}, its first step {first}"
        );
        // At a fixed scale the style zoom follows the parallel, which changes no faster than
        // tan(latitude) per radian, held at the Mercator world's last latitude.
        let next_zoom = state.zoom().value();
        let steepest = MERCATOR_LATITUDE_LIMIT.to_radians().tan() / std::f64::consts::LN_2;
        assert!(
            next_zoom.is_finite() && (next_zoom - zoom).abs() <= steepest * moved * 1.5,
            "step {step}: the zoom moves from {zoom} to {next_zoom}"
        );
        let flat = crate::render::projection::mercator_world_to_lat_lon(
            state.camera().position().x,
            state.camera().position().y,
            512.0 * 2_f64.powf(next_zoom),
        );
        assert!(flat.latitude.abs() <= MERCATOR_LATITUDE_LIMIT + 1e-9);
        highest = highest.max(latitude);
        crossed |= highest > 89.0 && latitude < 80.0;
        (previous, zoom) = (frame, next_zoom);
        if crossed {
            break;
        }
    }
    assert!(
        highest > 89.0 && crossed,
        "the drag never crossed the pole: {highest}"
    );
    // Beyond the pole the camera still faces the way it went: down the far side.
    let target = state.globe_pose().expect("pose").target;
    assert!(target[1] > 0.0 && target[1] < 80_f64.to_radians().sin());
}

#[test]
fn a_zoom_keeps_its_ground_under_the_pointer_over_the_cap() {
    let mut state = view(LatLon::new(84.0, 10.0), 5.0, 0.0, 40.0, 0.0);
    state
        .set_navigation_mode(NavigationMode::FreeGlobe, &VERTICAL)
        .expect("free navigation");
    let camera = globe_camera_for_view(&state).expect("camera");
    let pixel = camera.location_to_screen(LatLon::new(86.0, 15.0), 0.0);
    assert!(
        camera.is_point_on_map_surface(pixel),
        "the cap is in view at {pixel:?}"
    );
    let anchor = camera
        .screen_point_to_location_at(pixel, 0.0)
        .expect("ground");
    assert!(
        anchor.latitude > MERCATOR_LATITUDE_LIMIT,
        "the pointer is over the cap"
    );
    let distance = state.globe_pose().expect("pose").distance_meters;
    for _ in 0..4 {
        assert!(state.zoom_free_globe(pixel, 0.5));
    }
    let camera = globe_camera_for_view(&state).expect("camera");
    let at = camera.location_to_screen(anchor, 0.0);
    assert!(
        (at - pixel).x.hypot((at - pixel).y) < 1e-3,
        "the anchor shows at {at:?}"
    );
    let zoomed = state.globe_pose().expect("pose").distance_meters;
    assert!((zoomed / distance - 0.25).abs() < 1e-9);
}

#[test]
fn north_locked_navigation_ignores_the_free_gestures() {
    let mut state = view(LatLon::new(40.0, 0.0), 5.0, 0.0, 30.0, 0.0);
    let before = matrix(&state);
    assert!(!state.drag_free_globe(Point2::new(600.0, 400.0), Point2::new(600.0, 450.0)));
    assert!(!state.zoom_free_globe(Point2::new(600.0, 400.0), 1.0));
    assert_eq!(matrix(&state), before);
    assert_eq!(state.globe_pose(), None);
}

#[test]
fn a_pose_looking_straight_down_or_straight_up_rebuilds_its_orientation() {
    use cgmath::{Matrix3, Quaternion};
    // A pose turned past 90 degrees of pitch looks up from under its target, which no
    // camera draws, but the decomposition still has to give it back.
    for latitude in [0.0, 89.999_9, 90.0, -90.0] {
        for pitch in [0.0, 1e-9, 1e-7, 90.0, 179.999_999_9, 180.0] {
            for (bearing, roll) in [(0.0, 0.0), (137.0, 20.0), (-60.0, 250.0)] {
                let center = LatLon::new(latitude, 30.0);
                let rotation = super::view_rotation(center, bearing, pitch, roll);
                let free = super::FreeGlobe {
                    target: crate::projection::globe::lat_lon_to_unit_sphere(center),
                    orientation: Quaternion::from(rotation),
                    distance_meters: 50_000.0,
                };
                let (rebuilt_center, rebuilt_bearing, rebuilt_pitch, rebuilt_roll) =
                    free.decompose();
                let rebuilt: Matrix3<f64> = super::view_rotation(
                    rebuilt_center,
                    rebuilt_bearing,
                    rebuilt_pitch,
                    rebuilt_roll,
                );
                let case = format!("{latitude}, pitch {pitch}, bearing {bearing}, roll {roll}");
                for (column, expected) in [
                    (rebuilt.x, rotation.x),
                    (rebuilt.y, rotation.y),
                    (rebuilt.z, rotation.z),
                ] {
                    assert!(
                        (column - expected).magnitude() < 1e-7,
                        "{case}: {rebuilt:?} != {rotation:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn a_stored_pose_is_restored_only_where_the_projection_allows_free_navigation() {
    let mut state = view(LatLon::new(40.0, 0.0), 5.0, 0.0, 30.0, 0.0);
    state
        .set_navigation_mode(NavigationMode::FreeGlobe, &VERTICAL)
        .expect("free navigation");
    let pose = state.globe_pose().expect("pose");
    let mut other = view(LatLon::new(0.0, 0.0), 1.0, 0.0, 0.0, 0.0);
    assert!(matches!(
        other.restore_globe_pose(pose, &ProjectionType::Globe),
        Err(NavigationError::ProjectionNotSupported { .. })
    ));
    assert_eq!(other.navigation_mode(), NavigationMode::NorthLocked);
    other.restore_globe_pose(pose, &VERTICAL).expect("restore");
    assert_same_matrix(matrix(&other), matrix(&state), "restored");
}
