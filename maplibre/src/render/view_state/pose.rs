//! Camera pose: where the camera stands and where it looks.
//!
//! The view state keeps the map center, zoom and angles, as GL JS does. A pose gives the
//! camera's own ground position and altitude instead, the form a tracker or a flight path
//! provides. Converting a pose into center and zoom keeps tile covering and pixel scale
//! consistent with the map's own gestures, as GL JS `calculateCenterFromCameraLngLatAlt` and
//! `getCameraLngLat` do.

use std::f64::consts::PI;

use cgmath::{Deg, Point2, Rad};

use crate::{
    coords::{LatLon, Zoom, TILE_SIZE},
    render::view_state::ViewState,
    terrain::interaction::{
        camera_direction, camera_ground_position, distance_to_center_from_altitude,
    },
};

/// Where the camera stands and where it looks.
#[derive(Clone, Copy, Debug)]
pub struct CameraPose {
    /// Ground position under the camera.
    pub position: LatLon,
    /// Altitude of the camera in metres above sea level.
    pub altitude_meters: f64,
    /// Bearing, clockwise from north.
    pub bearing: Deg<f64>,
    /// Pitch away from looking straight down; beyond 90 degrees the camera looks upwards.
    pub pitch: Deg<f64>,
    /// Roll about the view axis.
    pub roll: Deg<f64>,
}

/// Bound on the center search, as GL JS sets it; the search converges in about five passes.
const MAX_CENTER_ITERATIONS: usize = 10;
/// Distance error in metres below which the center search stops.
const CENTER_TOLERANCE_METERS: f64 = 1e-12;

impl ViewState {
    /// Places the camera at `pose`.
    ///
    /// The map center moves to where the view ray meets the center elevation and the zoom
    /// follows the camera's distance to it, so a pose beyond the pitch limit is clamped to it.
    pub fn set_camera_pose(&mut self, pose: CameraPose) {
        let pitch = Rad::from(pose.pitch).0;
        let bearing = Rad::from(pose.bearing).0;
        let (distance_meters, elevation) =
            distance_to_center_from_altitude(pose.altitude_meters, self.center_elevation(), pitch);
        let (dx, dy, _) = camera_direction(pitch, bearing);
        let camera = mercator_from_lat_lon(pose.position);
        let circumference = self.body().circumference_meters();
        // The Mercator scale changes with latitude, and the center's latitude is the one that
        // matters; starting from the camera's scale, every pass re-evaluates it at the center
        // found so far.
        let mut meters_per_unit = circumference * latitude_at_mercator_y(camera.y).cos();
        let mut center = camera;
        let mut distance_units = distance_meters / meters_per_unit;
        for _ in 0..MAX_CENTER_ITERATIONS {
            distance_units = distance_meters / meters_per_unit;
            center = Point2::new(
                camera.x + dx * distance_units,
                camera.y + dy * distance_units,
            );
            meters_per_unit = circumference * latitude_at_mercator_y(center.y).cos();
            if (distance_meters - distance_units * meters_per_unit).abs() <= CENTER_TOLERANCE_METERS
            {
                break;
            }
        }
        let zoom = Zoom::new(
            (self.height()
                / 2.0
                / (self.field_of_view().0 / 2.0).tan()
                / distance_units
                / TILE_SIZE)
                .log2(),
        );
        let world_size = TILE_SIZE * 2f64.powf(zoom.value());
        self.update_zoom(zoom);
        self.set_center_elevation(elevation);
        let camera = self.camera_mut();
        camera.move_to(Point2::new(center.x * world_size, center.y * world_size));
        camera.set_bearing(pose.bearing);
        camera.set_pitch(pose.pitch);
        camera.set_roll(pose.roll);
    }

    /// Where the camera stands and where it looks.
    pub fn camera_pose(&self) -> CameraPose {
        let (position, altitude_meters) = camera_ground_position(self);
        let world_size = TILE_SIZE * 2f64.powf(self.zoom().value());
        let camera = self.camera();
        CameraPose {
            position: lat_lon_at_mercator(Point2::new(
                position.x / world_size,
                position.y / world_size,
            )),
            altitude_meters,
            bearing: camera.get_bearing().into(),
            pitch: camera.get_pitch().into(),
            roll: camera.get_roll().into(),
        }
    }
}

/// Position on the unit Mercator square, with y pointing south.
fn mercator_from_lat_lon(position: LatLon) -> Point2<f64> {
    let x = (position.longitude + 180.0) / 360.0;
    let latitude = position.latitude.to_radians();
    let y = 0.5 - (PI / 4.0 + latitude / 2.0).tan().ln() / (2.0 * PI);
    Point2::new(x, y)
}

fn lat_lon_at_mercator(point: Point2<f64>) -> LatLon {
    LatLon::new(
        latitude_at_mercator_y(point.y).to_degrees(),
        point.x * 360.0 - 180.0,
    )
}

/// Latitude in radians at a unit Mercator y.
fn latitude_at_mercator_y(y: f64) -> f64 {
    (PI * (1.0 - 2.0 * y)).sinh().atan()
}

#[cfg(test)]
mod tests;
