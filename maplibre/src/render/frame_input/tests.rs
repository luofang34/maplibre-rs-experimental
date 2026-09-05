#![allow(clippy::expect_used, clippy::panic)]

use std::time::Duration;

use cgmath::{Deg, Matrix4, Rad};

use super::{apply_frame_input, FrameInput, ViewSource};
use crate::{
    coords::{LatLon, WorldCoords, Zoom},
    render::view_state::{CameraPose, ExternalView, ExternalViewError, ViewState},
    window::PhysicalSize,
};

fn view_state(zoom: f64, position: LatLon) -> ViewState {
    let zoom = Zoom::new(zoom);
    ViewState::new(
        PhysicalSize::new(800, 600).expect("non-zero size"),
        WorldCoords::from_lat_lon(position, zoom),
        zoom,
        Deg(0.0),
        Rad(0.6435011087932844),
    )
}

/// A view state steered the way the gesture handlers steer it.
fn handled_view_state() -> ViewState {
    let mut view_state = view_state(10.0, LatLon::new(47.0, 11.0));
    view_state.set_camera_pose(CameraPose {
        position: LatLon::new(47.2, 11.3),
        altitude_meters: 2500.0,
        bearing: Deg(20.0),
        pitch: Deg(45.0),
        roll: Deg(0.0),
    });
    view_state
}

fn assert_matrices_close(a: Matrix4<f64>, b: Matrix4<f64>) {
    for column in 0..4 {
        for row in 0..4 {
            let (x, y) = (a[column][row], b[column][row]);
            assert!(
                (x - y).abs() <= 1e-9 * x.abs().max(y.abs()).max(1.0),
                "differs at column {column} row {row}: {x} vs {y}"
            );
        }
    }
}

#[test]
fn a_map_view_frame_renders_what_the_handlers_set() {
    let mut view_state = handled_view_state();
    let direct = view_state.view_projection().0;

    apply_frame_input(&FrameInput::default(), &mut view_state).expect("a map view always applies");

    assert_eq!(view_state.view_projection().0, direct);
}

#[test]
fn an_external_frame_drives_the_view_state_and_a_map_view_frame_releases_it() {
    let original = handled_view_state();
    let mut driven = view_state(3.0, LatLon::new(0.0, 0.0));
    let external = FrameInput {
        timestamp: Duration::from_millis(16),
        view: ViewSource::External(original.external_view()),
    };

    apply_frame_input(&external, &mut driven).expect("the map's own view applies");

    assert_matrices_close(driven.view_projection().0, original.view_projection().0);
    assert!(driven.external_projection().is_some());

    apply_frame_input(&FrameInput::default(), &mut driven).expect("a map view always applies");

    assert!(driven.external_projection().is_none());
    let pose = driven.camera_pose();
    assert!(
        (pose.altitude_meters - 2500.0).abs() < 1e-6,
        "the pose the external view left stays: {pose:?}"
    );
}

#[test]
fn a_rejected_external_view_leaves_the_view_state_alone() {
    let mut view_state = handled_view_state();
    let direct = view_state.view_projection().0;
    let broken = FrameInput {
        timestamp: Duration::ZERO,
        view: ViewSource::External(ExternalView {
            view: Matrix4::from_scale(0.0),
            ..view_state.external_view()
        }),
    };

    assert_eq!(
        apply_frame_input(&broken, &mut view_state),
        Err(ExternalViewError::SingularView)
    );
    assert_eq!(view_state.view_projection().0, direct);
}

#[test]
fn the_frame_clock_advances_without_overflowing() {
    let mut input = FrameInput::default();
    input.advance(Duration::from_millis(16));
    input.advance(Duration::from_millis(16));
    assert_eq!(input.timestamp, Duration::from_millis(32));

    input.timestamp = Duration::MAX;
    input.advance(Duration::from_secs(1));
    assert_eq!(input.timestamp, Duration::MAX);
}
