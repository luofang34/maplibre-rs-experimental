//! The free-globe pose that shows a center, zoom and angles, as the constrained camera would.

use cgmath::Quaternion;

use super::{view_rotation, GlobePose};
use crate::{
    projection::globe::{lat_lon_to_unit_sphere, scale},
    render::view_state::{CameraOptions, ViewState},
};

impl ViewState {
    /// The free-globe pose showing `options`, the camera's own center, zoom and angles standing
    /// in for those left out, as [`Self::jump_to`] places it. The center is not held within the
    /// Mercator world, so the pose may look at a pole.
    ///
    /// Its target height is the one [`Self::jump_elevation`] gives, or the center elevation
    /// for a jump that keeps it.
    pub fn globe_pose_for(&self, options: CameraOptions) -> GlobePose {
        let jump = self.resolve_jump(options);
        let rotation = view_rotation(jump.center, jump.bearing.0, jump.pitch.0, jump.roll.0);
        let orientation = Quaternion::from(rotation);
        // The inverse of the style zoom a pose reads back, near the poles too.
        let radius = scale::radius_pixels(jump.zoom.value(), jump.center.latitude);
        let target_elevation_meters = self
            .jump_elevation(options)
            .unwrap_or_else(|| self.center_elevation());
        GlobePose {
            target: lat_lon_to_unit_sphere(jump.center).into(),
            target_elevation_meters,
            orientation: [
                orientation.s,
                orientation.v.x,
                orientation.v.y,
                orientation.v.z,
            ],
            distance_meters: self.globe_camera_to_center_distance() / radius
                * self.body().radius_meters,
        }
    }
}

#[cfg(test)]
mod tests;
