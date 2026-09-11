use cgmath::{InnerSpace, Point2};

use super::{
    super::{camera::GlobeCameraState, covering::distance_to_tile_2d, unit_sphere_to_lat_lon},
    ZoomRounding,
};
use crate::coords::{LatLon, TileCoords, ZoomLevel, MAX_ZOOM};

const MAX_MERCATOR_HORIZON_DEGREES: f64 = 89.25;
/// Latitude beyond which Mercator has no coordinates.
const MAX_MERCATOR_LATITUDE_DEGREES: f64 = 85.051_129;
const MAX_ZOOM_LEVELS_ON_SCREEN: f64 = 9.314;
const TILE_COUNT_MAX_MIN_RATIO: f64 = 3.0;
const INTEGRATION_POINTS: usize = 10;

pub(crate) struct LodContext {
    camera: Point2<f64>,
    eye_focal_pixels: Option<f64>,
    distance_z: f64,
    distance_to_center_3d: f64,
    requested_zoom: f64,
    field_of_view_degrees: f64,
}

impl LodContext {
    pub(crate) fn new(camera: &GlobeCameraState, requested_zoom: f64) -> Self {
        if camera.is_external_eye() {
            // The center, pitch and distance an eye derives to describe where it looks, not
            // where it is: a level gaze derives to a pitch near ninety degrees, which would
            // put the camera a few percent of its height above the ground and have every
            // tile counted as seen at grazing incidence, levels coarser than the eye sees.
            let position = camera.camera_position();
            let beneath = unit_sphere_to_lat_lon(position);
            let height_radii = (position.magnitude() - 1.0).max(0.0);
            return Self::from_positions(
                mercator_point(beneath),
                mercator_center(camera),
                height_radii * radius_world_units(beneath.latitude),
                camera.field_of_view_degrees(),
                requested_zoom,
            );
        }
        Self::from_view(
            mercator_center(camera),
            camera.camera_to_center_distance() / camera.world_size(),
            camera.pitch_degrees(),
            camera.bearing_degrees(),
            camera.field_of_view_degrees(),
            requested_zoom,
        )
    }

    /// Builds the context from the map center in Mercator `0..1` units, the camera distance to
    /// it in the same units, and the camera angles in GL JS's conventions.
    pub(crate) fn from_view(
        center: Point2<f64>,
        distance: f64,
        pitch_degrees: f64,
        bearing_degrees: f64,
        field_of_view_degrees: f64,
        requested_zoom: f64,
    ) -> Self {
        let pitch = pitch_degrees.to_radians();
        let bearing = bearing_degrees.to_radians();
        let horizontal = pitch.sin();
        let direction_x = horizontal * bearing.sin();
        let direction_y = -horizontal * bearing.cos();
        let camera_point = Point2::new(
            center.x - distance * direction_x,
            center.y - distance * direction_y,
        );
        Self::from_positions(
            camera_point,
            center,
            distance * pitch.cos(),
            field_of_view_degrees,
            requested_zoom,
        )
    }

    /// Builds the context from the camera's and the center's positions in Mercator `0..1`
    /// units and the camera's height above the center in the same units.
    pub(crate) fn from_positions(
        camera: Point2<f64>,
        center: Point2<f64>,
        distance_z: f64,
        field_of_view_degrees: f64,
        requested_zoom: f64,
    ) -> Self {
        let distance_to_center_2d = (center.x - camera.x).hypot(center.y - camera.y);
        Self {
            camera,
            eye_focal_pixels: None,
            distance_z,
            distance_to_center_3d: distance_to_center_2d.hypot(distance_z),
            requested_zoom,
            field_of_view_degrees,
        }
    }

    pub(crate) fn from_eye(camera: Point2<f64>, height: f64, focal_pixels: f64) -> Self {
        Self {
            camera,
            distance_z: height,
            eye_focal_pixels: Some(focal_pixels),
            distance_to_center_3d: height,
            requested_zoom: 0.0,
            field_of_view_degrees: 0.0,
        }
    }

    #[cfg(test)]
    pub(crate) fn zoom_for_tile(&self, tile: TileCoords, rounding: ZoomRounding) -> ZoomLevel {
        self.stable_zoom_for_tile(tile, rounding, None)
    }

    pub(crate) fn stable_zoom_for_tile(
        &self,
        tile: TileCoords,
        rounding: ZoomRounding,
        history: Option<&crate::projection::lod_history::LodHistory>,
    ) -> ZoomLevel {
        let distance_2d = distance_to_tile_2d(self.camera, tile);
        let desired = if let Some(focal) = self.eye_focal_pixels {
            let distance = distance_2d.hypot(self.distance_z).max(f64::EPSILON);
            // Bound the longest projected texel axis. A grazing-angle area estimate
            // would erase cross-slope detail on terrain and blur roads toward the horizon.
            (focal / (crate::coords::TILE_SIZE * distance)).log2()
        } else {
            calculate_tile_zoom(
                self.requested_zoom,
                distance_2d,
                self.distance_z,
                self.distance_to_center_3d,
                self.field_of_view_degrees,
            )
        };
        let desired = rounding
            .apply(desired + history.map_or(0.0, |h| h.bias(tile)))
            .clamp(0.0, (MAX_ZOOM - 1) as f64);
        ZoomLevel::new(desired as u8)
    }
}

fn mercator_center(camera: &GlobeCameraState) -> Point2<f64> {
    mercator_point(camera.center())
}

/// A location in Mercator `0..1` units.
fn mercator_point(location: LatLon) -> Point2<f64> {
    let x = location.longitude / 360.0 + 0.5;
    let latitude = location
        .latitude
        .clamp(
            -MAX_MERCATOR_LATITUDE_DEGREES,
            MAX_MERCATOR_LATITUDE_DEGREES,
        )
        .to_radians();
    let y = (1.0 - latitude.tan().asinh() / std::f64::consts::PI) * 0.5;
    Point2::new(x, y)
}

/// The globe's radius in Mercator `0..1` units at a latitude, where a world unit spans the
/// circumference scaled by the Mercator stretch there.
fn radius_world_units(latitude_degrees: f64) -> f64 {
    let latitude = latitude_degrees
        .clamp(
            -MAX_MERCATOR_LATITUDE_DEGREES,
            MAX_MERCATOR_LATITUDE_DEGREES,
        )
        .to_radians();
    1.0 / (2.0 * std::f64::consts::PI * latitude.cos())
}

fn calculate_tile_zoom(
    requested_center_zoom: f64,
    distance_to_tile_2d: f64,
    distance_to_tile_z: f64,
    distance_to_center_3d: f64,
    field_of_view_degrees: f64,
) -> f64 {
    let pitch_behavior = pitch_tile_loading_behavior(field_of_view_degrees);
    let center_pitch = (distance_to_tile_z / distance_to_center_3d).acos();
    let half_fov = (field_of_view_degrees * 0.5).to_radians();
    let tile_count_pitch_zero = 2.0 * integral_cos_power(pitch_behavior - 1.0, 0.0, half_fov);
    let highest_pitch = (center_pitch + half_fov).min(MAX_MERCATOR_HORIZON_DEGREES.to_radians());
    let lowest_pitch = (center_pitch - half_fov).min(highest_pitch);
    let tile_count = integral_cos_power(pitch_behavior - 1.0, lowest_pitch, highest_pitch);
    let tile_pitch = (distance_to_tile_2d / distance_to_tile_z).atan();
    let distance_to_tile_3d = distance_to_tile_2d.hypot(distance_to_tile_z);
    let distance_scale = distance_to_center_3d / distance_to_tile_3d / 0.5_f64.max(half_fov.cos());
    requested_center_zoom + distance_scale.log2() + pitch_behavior * tile_pitch.cos().log2() * 0.5
        - (tile_count / tile_count_pitch_zero / TILE_COUNT_MAX_MIN_RATIO)
            .max(1.0)
            .log2()
            * 0.5
}

fn pitch_tile_loading_behavior(field_of_view_degrees: f64) -> f64 {
    let numerator = (MAX_MERCATOR_HORIZON_DEGREES - field_of_view_degrees)
        .to_radians()
        .cos();
    let denominator = MAX_MERCATOR_HORIZON_DEGREES.to_radians().cos();
    2.0 * ((MAX_ZOOM_LEVELS_ON_SCREEN - 1.0) / (numerator / denominator).log2() - 1.0)
}

fn integral_cos_power(power: f64, start: f64, end: f64) -> f64 {
    let width = (end - start) / INTEGRATION_POINTS as f64;
    (0..INTEGRATION_POINTS)
        .map(|index| {
            let x = start + (index as f64 + 0.5) * width;
            width * x.cos().powf(power)
        })
        .sum()
}
