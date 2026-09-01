//! Globe interaction rotations shared by input frontends.

use cgmath::{InnerSpace, Quaternion};

use super::{
    globe_zoom_adjustment, lat_lon_bearing_from_orientation, lat_lon_to_unit_sphere,
    orientation_from_lat_lon_bearing, wrap_longitude,
};
use crate::coords::LatLon;

const MIN_ROTATION_AXIS_SQUARED: f64 = 1e-24;

/// Camera-center change that keeps a geographic anchor under the cursor.
#[derive(Clone, Copy, Debug)]
pub struct GlobePanUpdate {
    /// Rotated geographic map center.
    pub center: LatLon,
    /// Zoom delta preserving the globe's apparent radius across latitude changes.
    pub zoom_adjustment: f64,
}

/// Rotates a globe so `anchor` moves to the surface location currently under the cursor.
///
/// The rotation uses the same surface-vector and quaternion frames as MapLibre GL JS. Bearing is
/// intentionally fixed for drag panning.
pub fn pan_center_to_anchor(
    current_center: LatLon,
    bearing_degrees: f64,
    anchor: LatLon,
    cursor_location: LatLon,
) -> GlobePanUpdate {
    let target = lat_lon_to_unit_sphere(anchor);
    let current = lat_lon_to_unit_sphere(cursor_location);
    let axis = target.cross(current);
    let axis_length_squared = axis.magnitude2();
    let dot = target.dot(current).clamp(-1.0, 1.0);
    let half_angle = dot.acos() * 0.5;
    let delta = if axis_length_squared > MIN_ROTATION_AXIS_SQUARED {
        let scaled_axis = axis * (half_angle.sin() / axis_length_squared.sqrt());
        Quaternion::new(
            half_angle.cos(),
            scaled_axis.y,
            -scaled_axis.x,
            scaled_axis.z,
        )
    } else {
        Quaternion::new(1.0, 0.0, 0.0, 0.0)
    };
    let orientation = orientation_from_lat_lon_bearing(current_center, bearing_degrees) * delta;
    let rotated = lat_lon_bearing_from_orientation(orientation);
    let center = LatLon::new(
        rotated.center.latitude.clamp(-90.0, 90.0),
        wrap_longitude(rotated.center.longitude),
    );

    GlobePanUpdate {
        center,
        zoom_adjustment: globe_zoom_adjustment(current_center.latitude, center.latitude),
    }
}

#[cfg(test)]
mod tests;
