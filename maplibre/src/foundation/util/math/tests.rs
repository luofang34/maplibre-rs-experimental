#![allow(clippy::expect_used)]

use cgmath::{InnerSpace, Point3, Vector3};

use super::{div_ceil, Aabb3, Plane};
use crate::coords::EXTENT_SINT;

#[test]
fn test_div_floor() {
    assert_eq!(div_ceil(7000, EXTENT_SINT), 2);
    assert_eq!(div_ceil(-7000, EXTENT_SINT), -1);
}

#[test]
fn point_normal_plane_intersects_its_defining_point() {
    for point in [Point3::new(1.0, 2.0, 3.0), Point3::new(-4.0, 1.0, -2.0)] {
        for normal in [
            Vector3::unit_z(),
            -Vector3::unit_z(),
            Vector3::new(2.0, 3.0, 4.0),
        ] {
            let origin = Vector3::new(point.x, point.y, point.z) - normal * 2.0;
            let plane = Plane::from_point_normal(point, normal);
            let distance = plane
                .intersection_distance_ray(&origin, &normal)
                .expect("ray crosses the plane");
            assert!((distance - 2.0_f64).abs() < 1e-12);
        }
    }
}

#[test]
fn plane_constructors_agree_on_box_intersections() {
    for height in [-3.0_f64, 2.0] {
        let bounds = Aabb3::new(
            Point3::new(1.0, 1.0, height - 1.0),
            Point3::new(4.0, 5.0, height + 1.0),
        );
        let point = Point3::new(1.0, 1.0, height);
        let through_points = Plane::from_points(
            point,
            Point3::new(4.0, 1.0, height),
            Point3::new(1.0, 5.0, height),
        )
        .expect("non-collinear points");
        let expected = through_points.intersection_points_aabb3(&bounds);
        assert_eq!(expected.len(), 4);
        for normal in [
            Vector3::unit_z(),
            -Vector3::unit_z(),
            Vector3::new(0.0, 0.0, 3.0),
        ] {
            let plane = Plane::from_point_normal(point, normal);
            let actual = plane.intersection_points_aabb3(&bounds);
            assert_eq!(actual.len(), expected.len());
            for (actual, expected) in actual.iter().zip(&expected) {
                assert!((*actual - *expected).magnitude() < 1e-12);
                assert!((actual.z - height).abs() < 1e-12);
            }
        }
    }
}
