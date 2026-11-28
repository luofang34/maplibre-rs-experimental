#![allow(clippy::expect_used, clippy::panic)]

use super::*;

#[test]
fn invalid_projections_return_errors_without_aborting() {
    for matrix in [
        Matrix4::zero(),
        Matrix4::from_scale(f64::NAN),
        Matrix4::from_scale(f64::INFINITY),
    ] {
        assert!(matches!(
            ViewProjection(matrix).invert(),
            Err(ViewProjectionError)
        ));
    }
}

#[test]
fn a_valid_projection_inverse_round_trips_clip_coordinates() {
    let view = ViewProjection(
        OPENGL_TO_WGPU_MATRIX * EyeFrustum::symmetric(Rad(1.4), 1.2, 0.05, 1.0e8).projection(),
    );
    let inverse = view.invert().expect("valid projection");
    for depth in [0.0, 0.5, 1.0] {
        let clip = Vector4::new(0.25, -0.5, depth, 1.0);
        let projected = view.project(inverse.project(clip));
        assert!((projected - clip).magnitude() < 1e-10);
    }
}
