#![allow(clippy::expect_used, clippy::panic)]

use cgmath::{perspective, Deg, Vector4};

use super::{OPENGL_TO_WGPU_MATRIX, REVERSED_Z};

fn depth_of(matrix: cgmath::Matrix4<f64>, view_z: f64) -> f64 {
    let clip = matrix * Vector4::new(0.0, 0.0, view_z, 1.0);
    clip.z / clip.w
}

#[test]
fn reversed_z_maps_near_plane_to_one_and_far_plane_to_zero() {
    let near = 12.0;
    let far = 90_000.0;
    let gpu = REVERSED_Z * OPENGL_TO_WGPU_MATRIX * perspective(Deg(36.87), 1.5, near, far);

    assert!((depth_of(gpu, -near) - 1.0).abs() < 1e-9);
    assert!(depth_of(gpu, -far).abs() < 1e-9);
    let middle = depth_of(gpu, -(near + far) / 2.0);
    assert!(middle > 0.0 && middle < 1.0);
}

#[test]
fn reversed_z_keeps_x_y_and_w() {
    let point = Vector4::new(3.0, -2.0, 0.25, 1.0);
    let flipped = REVERSED_Z * point;

    assert_eq!(flipped.x, point.x);
    assert_eq!(flipped.y, point.y);
    assert_eq!(flipped.w, point.w);
    assert_eq!(flipped.z, 1.0 - point.z);
}
