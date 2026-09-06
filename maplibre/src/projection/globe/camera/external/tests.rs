#![allow(clippy::expect_used, clippy::panic)]

use cgmath::{InnerSpace, Matrix4, Point2, Vector3, Vector4};

use super::ExternalGlobeEye;
use crate::{
    coords::LatLon,
    projection::{
        body::Body,
        globe::camera::{GlobeCameraError, GlobeCameraOptions, GlobeCameraState},
    },
    render::camera::EyeFrustum,
};

fn posed_options() -> GlobeCameraOptions {
    GlobeCameraOptions {
        width: 1024.0,
        height: 768.0,
        field_of_view_degrees: 45.0,
        center: LatLon::new(47.0, 11.0),
        world_size: 512.0 * 8.0,
        bearing_degrees: 35.0,
        pitch_degrees: 40.0,
        roll_degrees: 12.0,
        center_offset: Point2::new(0.0, 0.0),
        body: Body::EARTH,
    }
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

fn assert_vectors_close(a: Vector4<f64>, b: Vector4<f64>, what: &str) {
    for index in 0..4 {
        assert!(
            (a[index] - b[index]).abs() <= 1e-9,
            "{what} differs at {index}: {a:?} vs {b:?}"
        );
    }
}

/// The eye the map's own globe camera stands for.
fn eye_of(camera: &GlobeCameraState) -> ExternalGlobeEye {
    let (near, far) = camera.depth_range();
    ExternalGlobeEye::from_view(
        camera.view(),
        EyeFrustum::from_projection(camera.projection(), near, far),
        camera.camera_to_center_distance(),
    )
    .expect("a globe view matrix is invertible")
}

#[test]
fn the_maps_own_globe_camera_survives_the_round_trip_through_an_eye() {
    let native = GlobeCameraState::new(posed_options()).expect("valid options");

    let rebuilt = GlobeCameraState::from_external_eye(posed_options(), eye_of(&native))
        .expect("the map's own eye is above the surface");

    assert_matrices_close(rebuilt.view(), native.view(), "view");
    assert_matrices_close(rebuilt.projection(), native.projection(), "projection");
    assert_matrices_close(
        rebuilt.wgpu_view_projection(),
        native.wgpu_view_projection(),
        "view projection",
    );
    assert_vectors_close(
        rebuilt.camera_position().extend(0.0),
        native.camera_position().extend(0.0),
        "camera position",
    );
    assert_vectors_close(
        rebuilt.clipping_plane(),
        native.clipping_plane(),
        "clipping plane",
    );
    assert_eq!(rebuilt.depth_range(), native.depth_range());
    assert_eq!(
        rebuilt.camera_to_center_distance(),
        native.camera_to_center_distance()
    );
    assert_eq!(rebuilt.globe_radius_pixels(), native.globe_radius_pixels());
}

#[test]
fn an_eye_beside_the_globe_faces_its_own_horizon() {
    let native = GlobeCameraState::new(posed_options()).expect("valid options");
    let eye = ExternalGlobeEye {
        position: Vector3::new(2.5, 0.0, 0.0),
        axes: eye_of(&native).axes,
        ..eye_of(&native)
    };

    let camera =
        GlobeCameraState::from_external_eye(posed_options(), eye).expect("above the surface");

    let plane = camera.clipping_plane();
    assert_vectors_close(plane, Vector4::new(1.0, 0.0, 0.0, -0.4), "clipping plane");
    let behind = Vector3::new(-1.0, 0.0, 0.0).extend(1.0);
    assert!(
        plane.dot(behind) < 0.0,
        "the far side is behind the horizon"
    );
    let facing = Vector3::new(1.0, 0.0, 0.0).extend(1.0);
    assert!(plane.dot(facing) > 0.0, "the near side faces the eye");
    assert_eq!(camera.camera_position(), eye.position);
}

#[test]
fn an_eye_at_or_below_the_surface_is_refused() {
    let native = GlobeCameraState::new(posed_options()).expect("valid options");
    for length in [1.0, 0.5, 0.0] {
        let eye = ExternalGlobeEye {
            position: Vector3::new(0.0, 0.0, length),
            ..eye_of(&native)
        };
        match GlobeCameraState::from_external_eye(posed_options(), eye) {
            Err(GlobeCameraError::EyeBelowSurface { distance }) => assert_eq!(distance, length),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }
}

#[test]
fn an_unbounded_far_plane_falls_back_to_the_globe_extent() {
    let native = GlobeCameraState::new(posed_options()).expect("valid options");
    let mut eye = eye_of(&native);
    eye.frustum.far = f64::INFINITY;

    let camera =
        GlobeCameraState::from_external_eye(posed_options(), eye).expect("above the surface");

    let (near, far) = camera.depth_range();
    assert_eq!(near, eye.frustum.near);
    assert_eq!(
        far,
        native.camera_to_center_distance() + 2.0 * native.globe_radius_pixels()
    );
    assert!(camera.projection().invert_is_finite());
}

trait InvertIsFinite {
    fn invert_is_finite(&self) -> bool;
}

impl InvertIsFinite for Matrix4<f64> {
    fn invert_is_finite(&self) -> bool {
        cgmath::SquareMatrix::invert(self)
            .is_some_and(|inverse| (0..4).all(|c| (0..4).all(|r| inverse[c][r].is_finite())))
    }
}

#[test]
fn a_singular_view_places_no_eye() {
    let native = GlobeCameraState::new(posed_options()).expect("valid options");
    let (near, far) = native.depth_range();
    assert!(ExternalGlobeEye::from_view(
        Matrix4::from_scale(0.0),
        EyeFrustum::from_projection(native.projection(), near, far),
        native.camera_to_center_distance(),
    )
    .is_none());
}
