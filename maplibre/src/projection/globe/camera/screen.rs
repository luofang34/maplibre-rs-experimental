//! Rays from viewport pixels and where they meet the globe.

use cgmath::{InnerSpace, Point2, Vector3, Vector4};

use super::{GlobeCameraState, MIN_DIRECTION_LENGTH_SQUARED};
use crate::{
    coords::LatLon,
    projection::globe::{
        closest_point_on_sphere, horizon_plane_to_circle, ray_sphere_intersection,
        unit_sphere_to_lat_lon,
    },
};

const HORIZON_FALLBACK_RAY_LENGTH: f64 = 2.0;

impl GlobeCameraState {
    /// Returns a normalized world-space ray from the camera through a viewport pixel.
    pub fn ray_direction_from_pixel(&self, pixel: Point2<f64>) -> Option<Vector3<f64>> {
        let clip = Vector4::new(
            pixel.x / self.options.width * 2.0 - 1.0,
            -(pixel.y / self.options.height * 2.0 - 1.0),
            1.0,
            1.0,
        );
        let world = self.inverse_view_projection * clip;
        if world.w.abs() <= f64::EPSILON {
            return None;
        }
        let point = world.truncate() / world.w;
        let direction = point - self.camera_position;
        (direction.magnitude2() > MIN_DIRECTION_LENGTH_SQUARED).then(|| direction.normalize())
    }

    /// Converts a viewport pixel to a surface location, clamping misses to the visible horizon.
    pub fn screen_point_to_location(&self, pixel: Point2<f64>) -> Option<LatLon> {
        let direction = self.ray_direction_from_pixel(pixel)?;
        if let Some(intersection) = ray_sphere_intersection(self.camera_position, direction, 1.0) {
            let point = self.camera_position + direction * intersection.t_min;
            if point.magnitude2() > MIN_DIRECTION_LENGTH_SQUARED {
                return Some(unit_sphere_to_lat_lon(point.normalize()));
            }
        }
        self.closest_horizon_location(direction)
    }

    /// Returns whether the ray through a viewport pixel intersects the unit globe.
    pub fn is_point_on_map_surface(&self, pixel: Point2<f64>) -> bool {
        self.ray_direction_from_pixel(pixel)
            .is_some_and(|direction| {
                ray_sphere_intersection(self.camera_position, direction, 1.0).is_some()
            })
    }

    fn closest_horizon_location(&self, direction: Vector3<f64>) -> Option<LatLon> {
        let normal = self.clipping_plane.truncate();
        let denominator = normal.dot(direction);
        let origin_distance = normal.dot(self.camera_position) + self.clipping_plane.w;
        let distance = if denominator.abs() > f64::EPSILON {
            -origin_distance / denominator
        } else {
            -1.0
        };
        let plane_point = if distance.is_finite() && distance > 0.0 {
            self.camera_position + direction * distance
        } else {
            let distant = self.camera_position + direction * HORIZON_FALLBACK_RAY_LENGTH;
            let plane_distance = normal.dot(distant) + self.clipping_plane.w;
            distant - normal * plane_distance
        };
        let horizon = horizon_plane_to_circle(self.clipping_plane);
        closest_point_on_sphere(horizon.center, horizon.radius, plane_point)
            .or_else(|| (horizon.radius == 0.0).then_some(horizon.center))
            .map(unit_sphere_to_lat_lon)
    }
}
