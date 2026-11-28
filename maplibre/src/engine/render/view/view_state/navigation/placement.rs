//! The free-globe pose that shows a center, zoom and angles, as the constrained camera would.

use cgmath::Quaternion;

use super::{view_rotation, GlobePose};
use crate::{
    coords::{LatLon, Zoom},
    projection::globe::{lat_lon_to_unit_sphere, scale},
    render::view_state::{CameraOptions, ViewState},
};

/// The center, zoom and angles a jump resolves to.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ResolvedJump {
    pub center: LatLon,
    pub zoom: Zoom,
    pub bearing: cgmath::Deg<f64>,
    pub pitch: cgmath::Deg<f64>,
    pub roll: cgmath::Deg<f64>,
}

impl ResolvedJump {
    /// Whether two cameras show the same, but for rounding.
    pub(crate) fn same_as(&self, other: &Self) -> bool {
        const TOLERANCE: f64 = 1e-9;
        [
            self.center.latitude - other.center.latitude,
            self.center.longitude - other.center.longitude,
            self.zoom.value() - other.zoom.value(),
            self.bearing.0 - other.bearing.0,
            self.pitch.0 - other.pitch.0,
            self.roll.0 - other.roll.0,
        ]
        .iter()
        .all(|difference| difference.abs() <= TOLERANCE)
    }
}

impl ViewState {
    /// The height a jump to `options` rests its center at, `None` keeping the current one:
    /// the elevation given, or, for a center that moves without one, sea level where terrain
    /// holds it, as GL JS reads ground it has not loaded, and the center altitude otherwise.
    /// A new center never inherits the ground of the one it left.
    pub fn jump_elevation(&self, options: CameraOptions) -> Option<f64> {
        options
            .elevation
            .filter(|meters| meters.is_finite())
            .or_else(|| {
                options.center.map(|_| {
                    if self.center_held_by_terrain() {
                        0.0
                    } else {
                        self.center_altitude()
                    }
                })
            })
    }

    /// The center, zoom and angles a jump to `options` shows, the camera's own standing in for
    /// those left out.
    pub(crate) fn resolve_jump(&self, options: CameraOptions) -> ResolvedJump {
        let camera = self.camera();
        // A free-globe camera may look past the Mercator world the flat center stops at.
        let raw_center = options.center.unwrap_or_else(|| {
            self.pose_view()
                .map_or_else(|| self.center_lat_lon(), |view| view.center)
        });
        let raw_zoom = options
            .zoom
            .filter(|zoom| zoom.is_finite())
            .map_or_else(|| self.zoom(), Zoom::new);
        ResolvedJump {
            center: self.clamp_center(raw_center),
            zoom: self.clamp_zoom(raw_zoom),
            bearing: options
                .bearing
                .unwrap_or_else(|| camera.get_bearing().into()),
            pitch: options.pitch.unwrap_or_else(|| camera.get_pitch().into()),
            roll: options.roll.unwrap_or_else(|| camera.get_roll().into()),
        }
    }
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
