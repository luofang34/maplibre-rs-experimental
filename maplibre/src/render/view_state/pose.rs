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

/// How many heights ahead an eye's center may lie; beyond that the ground near the horizon
/// would sling the center and the zoom by kilometres per degree of head pitch, and a glance
/// up would request tiles at a wildly different zoom.
const EYE_CENTER_REACH_HEIGHTS: f64 = 5.0;
/// The reach in metres is kept within these bounds: near the ground the center stays far
/// enough ahead that the covering reaches the horizon, and from high up it stays near enough
/// that the center's latitude remains on the map.
const MIN_EYE_CENTER_REACH_METERS: f64 = 10_000.0;
const MAX_EYE_CENTER_REACH_METERS: f64 = 50_000.0;

impl ViewState {
    /// Places the camera at `pose`.
    ///
    /// The map center moves to where the view ray meets the center elevation and the zoom
    /// follows the camera's distance to it, so a pose beyond the pitch limit is clamped to it.
    pub fn set_camera_pose(&mut self, pose: CameraPose) {
        let pitch = Rad::from(pose.pitch).0;
        let (distance_meters, elevation) =
            distance_to_center_from_altitude(pose.altitude_meters, self.center_elevation(), pitch);
        self.place_camera(pose, distance_meters, elevation);
    }

    /// Places a host's eye at `pose`. The center is where the gaze meets the center elevation,
    /// at most `EYE_CENTER_REACH_HEIGHTS` heights ahead; a gaze nearer the horizon than that
    /// keeps the center at that reach and leaves the center elevation alone, so a head
    /// turning up to the horizon moves the center smoothly instead of snapping it to a fixed
    /// distance in the air.
    pub(super) fn set_eye_pose(&mut self, pose: CameraPose) {
        let pitch = Rad::from(pose.pitch).0;
        let dz = -pitch.cos();
        let elevation = self.center_elevation();
        let above_ground = pose.altitude_meters - elevation;
        let reach = (EYE_CENTER_REACH_HEIGHTS * above_ground.abs())
            .clamp(MIN_EYE_CENTER_REACH_METERS, MAX_EYE_CENTER_REACH_METERS);
        let distance_meters = if dz * above_ground < 0.0 {
            (-above_ground / dz).min(reach)
        } else {
            reach
        };
        self.place_camera(pose, distance_meters, elevation);
    }

    fn place_camera(&mut self, pose: CameraPose, distance_meters: f64, elevation: f64) {
        let pitch = Rad::from(pose.pitch).0;
        let bearing = Rad::from(pose.bearing).0;
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
        let zoom = self.zoom_for_center_distance_units(distance_units);
        self.set_center_elevation(elevation);
        self.set_center_and_angles(
            lat_lon_at_mercator(center),
            zoom,
            pose.bearing,
            pose.pitch,
            pose.roll,
        );
    }

    /// The zoom at which the camera sits `distance_meters` from a center at `center`, the
    /// definition a pose and an external eye share.
    pub(super) fn zoom_for_center_distance(&self, distance_meters: f64, center: LatLon) -> Zoom {
        let meters_per_unit =
            self.body().circumference_meters() * center.latitude.to_radians().cos();
        self.zoom_for_center_distance_units(distance_meters / meters_per_unit)
    }

    /// The zoom at which the camera sits `distance_units` Mercator units from the center.
    fn zoom_for_center_distance_units(&self, distance_units: f64) -> Zoom {
        Zoom::new(
            (self.height()
                / 2.0
                / (self.field_of_view().0 / 2.0).tan()
                / distance_units
                / TILE_SIZE)
                .log2(),
        )
    }

    /// Moves the map center and the camera angles, leaving the center elevation as it is.
    pub(super) fn set_center_and_angles(
        &mut self,
        center: LatLon,
        zoom: Zoom,
        bearing: Deg<f64>,
        pitch: Deg<f64>,
        roll: Deg<f64>,
    ) {
        let world_size = TILE_SIZE * 2f64.powf(zoom.value());
        let center = mercator_from_lat_lon(center);
        self.update_zoom(zoom);
        let camera = self.camera_mut();
        camera.move_to(Point2::new(center.x * world_size, center.y * world_size));
        camera.set_bearing(bearing);
        camera.set_pitch(pitch);
        camera.set_roll(roll);
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
pub(super) fn mercator_from_lat_lon(position: LatLon) -> Point2<f64> {
    let x = (position.longitude + 180.0) / 360.0;
    let latitude = position.latitude.to_radians();
    let y = 0.5 - (PI / 4.0 + latitude / 2.0).tan().ln() / (2.0 * PI);
    Point2::new(x, y)
}

pub(super) fn lat_lon_at_mercator(point: Point2<f64>) -> LatLon {
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
