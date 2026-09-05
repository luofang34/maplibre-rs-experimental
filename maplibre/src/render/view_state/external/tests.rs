#![allow(clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;

use cgmath::{Deg, Matrix4, Rad, SquareMatrix};

use super::{ExternalView, ExternalViewError};
use crate::{
    coords::{LatLon, WorldCoords, WorldTileCoords, Zoom, ZoomLevel},
    render::{
        camera::{EdgeInsets, FLIP_Y, OPENGL_TO_WGPU_MATRIX},
        view_state::{CameraPose, ViewState, ViewStatePadding},
    },
    window::PhysicalSize,
};

const FOVY: Rad<f64> = Rad(0.6435011087932844);

fn view_state(zoom: f64, position: LatLon) -> ViewState {
    let zoom = Zoom::new(zoom);
    let mut view_state = ViewState::new(
        PhysicalSize::new(800, 600).expect("non-zero size"),
        WorldCoords::from_lat_lon(position, zoom),
        zoom,
        Deg(0.0),
        FOVY,
    );
    view_state.set_max_pitch(Deg(180.0));
    view_state
}

fn posed_view_state() -> ViewState {
    let mut view_state = view_state(10.0, LatLon::new(47.0, 11.0));
    view_state.set_center_elevation(650.0);
    view_state.set_camera_pose(CameraPose {
        position: LatLon::new(47.3, 11.4),
        altitude_meters: 3000.0,
        bearing: Deg(35.0),
        pitch: Deg(55.0),
        roll: Deg(12.0),
    });
    view_state
}

fn assert_matrices_close(a: Matrix4<f64>, b: Matrix4<f64>, what: &str) {
    for column in 0..4 {
        for row in 0..4 {
            let (x, y) = (a[column][row], b[column][row]);
            assert!(
                (x - y).abs() <= 1e-9 * x.abs().max(y.abs()).max(1.0),
                "{what} differs at column {column} row {row}: {x} vs {y}"
            );
        }
    }
}

fn covering(view_state: &ViewState) -> BTreeSet<WorldTileCoords> {
    view_state
        .create_view_region(ZoomLevel::new(12), ViewStatePadding::Tight)
        .expect("a view region")
        .iter()
        .collect()
}

#[test]
fn injecting_the_maps_own_matrices_reproduces_its_frame_and_covering() {
    let original = posed_view_state();
    let mut injected = view_state(4.0, LatLon::new(0.0, 0.0));
    injected.set_center_elevation(650.0);

    injected
        .set_external_view(original.external_view())
        .expect("the map's own view is invertible");

    let (a, b) = (original.camera_pose(), injected.camera_pose());
    assert!(
        (a.position.latitude - b.position.latitude).abs() < 1e-9,
        "{a:?} vs {b:?}"
    );
    assert!(
        (a.position.longitude - b.position.longitude).abs() < 1e-9,
        "{a:?} vs {b:?}"
    );
    assert!(
        (a.altitude_meters - b.altitude_meters).abs() < 1e-6,
        "{a:?} vs {b:?}"
    );
    assert!((a.bearing.0 - b.bearing.0).abs() < 1e-9, "{a:?} vs {b:?}");
    assert!((a.pitch.0 - b.pitch.0).abs() < 1e-9, "{a:?} vs {b:?}");
    assert!((a.roll.0 - b.roll.0).abs() < 1e-9, "{a:?} vs {b:?}");
    assert!((original.zoom().value() - injected.zoom().value()).abs() < 1e-9);
    assert_matrices_close(
        original.view_projection().0,
        injected.view_projection().0,
        "view projection",
    );
    assert_eq!(covering(&original), covering(&injected));
}

#[test]
fn an_off_center_perspective_survives_the_round_trip() {
    let mut original = posed_view_state();
    original.set_edge_insets(EdgeInsets {
        top: 120.0,
        bottom: 0.0,
        left: 40.0,
        right: 0.0,
    });
    let mut injected = view_state(4.0, LatLon::new(0.0, 0.0));
    injected.set_center_elevation(650.0);

    injected
        .set_external_view(original.external_view())
        .expect("the map's own view is invertible");

    assert_matrices_close(
        original.view_projection().0,
        injected.view_projection().0,
        "view projection",
    );
}

#[test]
fn a_per_eye_projection_is_used_as_it_is_and_moves_the_covering() {
    let original = posed_view_state();
    let symmetric = original.external_view();
    // A frustum sheared far to one side, as an eye of a stereo pair would be.
    let eye = ExternalView {
        projection: Matrix4::from_translation(cgmath::Vector3::new(1.5, 0.0, 0.0))
            * symmetric.projection,
        ..symmetric
    };
    let mut injected = view_state(4.0, LatLon::new(0.0, 0.0));
    injected.set_center_elevation(650.0);

    injected.set_external_view(eye).expect("invertible");

    let camera_only = injected.view_projection().0;
    let expected = OPENGL_TO_WGPU_MATRIX * eye.projection * FLIP_Y * {
        // The camera matrix is what the map's own perspective would be applied to.
        let mut own = injected.clone();
        own.clear_external_view();
        (FLIP_Y * OPENGL_TO_WGPU_MATRIX * (FLIP_Y * symmetric.projection * FLIP_Y))
            .invert()
            .expect("invertible")
            * own.view_projection().0
    };
    assert_matrices_close(camera_only, expected, "sheared view projection");
    assert_ne!(covering(&original), covering(&injected));
    assert_eq!(injected.external_projection(), Some(eye.projection));

    injected.clear_external_view();
    assert_eq!(injected.external_projection(), None);
    assert_matrices_close(
        original.view_projection().0,
        injected.view_projection().0,
        "the map's own perspective returns",
    );
}

#[test]
fn a_view_pitched_beyond_the_limit_is_refused_rather_than_clamped() {
    let original = posed_view_state();
    let mut limited = view_state(4.0, LatLon::new(0.0, 0.0));
    limited.set_max_pitch(Deg(50.0));
    let before = limited.view_projection().0;

    let error = limited
        .set_external_view(original.external_view())
        .expect_err("a 55 degree pitch exceeds a 50 degree limit");

    match error {
        ExternalViewError::PitchBeyondLimit { pitch, max_pitch } => {
            assert!((pitch.0 - 55.0).abs() < 1e-9, "{pitch:?}");
            assert_eq!(max_pitch, Deg(50.0));
        }
        other => panic!("unexpected error {other:?}"),
    }
    assert_eq!(limited.view_projection().0, before);
    assert_eq!(limited.external_projection(), None);
}

#[test]
fn a_singular_view_matrix_is_refused() {
    let mut view_state = posed_view_state();
    let external = ExternalView {
        view: Matrix4::from_scale(0.0),
        ..view_state.external_view()
    };
    assert_eq!(
        view_state.set_external_view(external),
        Err(ExternalViewError::SingularView)
    );
    assert_eq!(view_state.external_projection(), None);
}
