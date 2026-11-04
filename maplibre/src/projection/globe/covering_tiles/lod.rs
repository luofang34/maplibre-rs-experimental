use cgmath::{InnerSpace, Point2, Vector3};

use super::{
    super::{
        camera::GlobeCameraState,
        covering::{
            closest_mercator_data_point_on_tile, closest_unit_sphere_point_on_tile,
            distance_to_tile_2d,
        },
    },
    ZoomRounding,
};
#[cfg(test)]
use crate::coords::LatLon;
use crate::coords::{TileCoords, ZoomLevel, MAX_ZOOM};

const MAX_MERCATOR_HORIZON_DEGREES: f64 = 89.25;
/// Latitude beyond which Mercator has no coordinates.
const MAX_MERCATOR_LATITUDE_DEGREES: f64 = 85.051_129;
const MAX_ZOOM_LEVELS_ON_SCREEN: f64 = 9.314;
const TILE_COUNT_MAX_MIN_RATIO: f64 = 3.0;
const INTEGRATION_POINTS: usize = 10;
/// The vertical field of a map camera, `2 * atan(0.75)`, as in GL JS.
const NORMAL_FIELD_OF_VIEW_DEGREES: f64 = 36.869_897_645_844_02;

#[derive(Clone, Copy)]
pub(crate) struct LodContext {
    camera: Point2<f64>,
    eye_sphere: Option<(Vector3<f64>, f64)>,
    eye_focal_pixels: Option<f64>,
    distance_z: f64,
    distance_to_center_3d: f64,
    requested_zoom: f64,
    field_of_view_degrees: f64,
    /// Whether a tile's distance is the straight distance to its rectangle, as on the Mercator
    /// plane, instead of the farther of the two axis distances GL JS uses on the globe.
    planar: bool,
}

impl LodContext {
    pub(crate) fn new(camera: &GlobeCameraState, requested_zoom: f64) -> Self {
        // Measure distance and foreshortening directly on S^2 from the camera's 3D unit-globe
        // position and SO(3) focus direction so orbital, FreeGlobe, and external-eye cameras
        // are all singularity-free across ±90° poles and ±180° antimeridian without Mercator dx.
        let position = camera.camera_position();
        let len = position.magnitude();
        let eye_hat = if len > f64::EPSILON {
            position / len
        } else {
            Vector3::unit_z()
        };
        let target_radius = if camera.is_external_eye() {
            1.0
        } else {
            camera.target().magnitude()
        };
        let min_clearance = 1.0 / camera.body().radius_meters;
        let height_radii = (len - target_radius).max(min_clearance);
        let focus_chord = (eye_hat - camera.focus_unit_sphere()).magnitude();
        Self {
            camera: Point2::new(0.5, 0.5),
            eye_sphere: Some((eye_hat, height_radii)),
            eye_focal_pixels: None,
            distance_z: height_radii,
            distance_to_center_3d: focus_chord.hypot(height_radii),
            requested_zoom,
            field_of_view_degrees: camera.field_of_view_degrees(),
            planar: false,
        }
    }

    /// The level whose texels match the screen's pixels where the view meets each tile, seen
    /// from where `camera` actually stands, for a viewport `viewport_height` pixels tall.
    /// `level_offset` shifts every level, as a source's tile size shifts its request.
    ///
    /// A free-globe pose takes this instead of GL JS's pitch rule: a gaze at the horizon
    /// derives a pitch near ninety degrees, at which that rule drops every distant tile
    /// several levels for foreshortening, though a horizon's silhouette runs across the
    /// line of sight and needs the full detail its pixels resolve.
    pub(crate) fn screen_resolution(
        camera: &GlobeCameraState,
        viewport_height: f64,
        level_offset: f64,
    ) -> Self {
        let focal =
            0.5 * viewport_height / (0.5 * camera.field_of_view_degrees()).to_radians().tan();
        Self::screen_resolution_focal(camera, focal, level_offset, 0.0)
    }

    /// Screen-pixel texel resolution using an explicit focal length in pixels (such as an
    /// [`ExternalView`](crate::render::view_state::ExternalView)'s `lod_focal_pixels`) and local
    /// ground elevation beneath the eye.
    pub(crate) fn screen_resolution_focal(
        camera: &GlobeCameraState,
        focal_pixels: f64,
        level_offset: f64,
        ground_elevation_meters: f64,
    ) -> Self {
        let position = camera.camera_position();
        let len = position.magnitude();
        let eye_hat = if len > f64::EPSILON {
            position / len
        } else {
            Vector3::unit_z()
        };
        let body = camera.body();
        let ground_radius = body.unit_radius_at(ground_elevation_meters);
        let min_clearance = 1.0 / body.radius_meters;
        let height_radii = (len - ground_radius).max(min_clearance);
        let parallel_cos = parallel_cos_from_sin_lat(eye_hat.y);
        let height = height_radii / (std::f64::consts::TAU * parallel_cos);
        Self {
            camera: Point2::new(0.5, 0.5),
            eye_sphere: Some((eye_hat, height_radii)),
            distance_z: height,
            eye_focal_pixels: Some(focal_pixels * 2_f64.powf(level_offset)),
            distance_to_center_3d: height,
            requested_zoom: 0.0,
            field_of_view_degrees: 0.0,
            planar: false,
        }
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
            eye_sphere: None,
            eye_focal_pixels: None,
            distance_z,
            distance_to_center_3d: distance_to_center_2d.hypot(distance_z),
            requested_zoom,
            field_of_view_degrees,
            planar: false,
        }
    }

    /// Measures the distance to a tile as the straight distance to its rectangle.
    pub(crate) fn planar(mut self) -> Self {
        self.planar = true;
        self
    }

    pub(crate) fn from_eye(camera: Point2<f64>, height: f64, focal_pixels: f64) -> Self {
        Self {
            camera,
            eye_sphere: None,
            distance_z: height,
            eye_focal_pixels: Some(focal_pixels),
            distance_to_center_3d: height,
            requested_zoom: 0.0,
            field_of_view_degrees: 0.0,
            planar: false,
        }
    }

    /// The context of a copy of the world `worlds` over: the camera lies that many worlds the
    /// other way from its tiles.
    pub(crate) fn shifted(&self, worlds: f64) -> Self {
        Self {
            camera: Point2::new(self.camera.x - worlds, self.camera.y),
            ..*self
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
        let desired = rounding
            .apply(self.desired_zoom(tile) + history.map_or(0.0, |h| h.bias(tile)))
            .clamp(0.0, (MAX_ZOOM - 1) as f64);
        ZoomLevel::new(desired as u8)
    }

    /// The fractional level `tile` should be drawn at, before rounding.
    fn desired_zoom(&self, tile: TileCoords) -> f64 {
        if !self.planar {
            if let Some((eye_hat, height_radii)) = self.eye_sphere {
                if let Some(focal) = self.eye_focal_pixels {
                    let (q_hat, _) = closest_unit_sphere_point_on_tile(eye_hat, tile);
                    let chord_vec = eye_hat - q_hat;
                    let d_radii_3d = (chord_vec + eye_hat * height_radii).magnitude();
                    let parallel_cos = if u8::from(tile.z) == 0 {
                        1.0
                    } else {
                        parallel_cos_from_sin_lat(q_hat.y)
                    };
                    let distance =
                        (d_radii_3d / (std::f64::consts::TAU * parallel_cos)).max(f64::EPSILON);
                    return (focal / (crate::coords::TILE_SIZE * distance)).log2();
                }
                // Source tiles carry no raster/DEM/MVT texels or vertices poleward of ±85.051129°
                // (the polar fan to ±90° is synthetic), so measure S^2 distance to the tile's
                // actual Web Mercator data domain rather than demanding 2^z empty polar wedges.
                let (_, chebyshev_chord) = closest_mercator_data_point_on_tile(eye_hat, tile);
                return calculate_tile_zoom(
                    self.requested_zoom,
                    chebyshev_chord,
                    self.distance_z,
                    self.distance_to_center_3d,
                    self.field_of_view_degrees,
                );
            }
        }
        let distance_2d = if self.planar {
            distance_to_tile_planar(self.camera, tile)
        } else {
            distance_to_tile_2d(self.camera, tile)
        };
        if let Some(focal) = self.eye_focal_pixels {
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
        }
    }
}

fn parallel_cos_from_sin_lat(sin_lat: f64) -> f64 {
    let min_cos = MAX_MERCATOR_LATITUDE_DEGREES.to_radians().cos();
    (1.0 - sin_lat * sin_lat).max(0.0).sqrt().max(min_cos)
}

/// The straight distance from a point to a tile's rectangle in Mercator `0..1` units.
fn distance_to_tile_planar(point: Point2<f64>, tile: TileCoords) -> f64 {
    let scale = 2_f64.powi(i32::from(u8::from(tile.z)));
    let size = 1.0 / scale;
    let axis = |point: f64, corner: f64| {
        let delta = point - corner;
        if delta < 0.0 {
            -delta
        } else {
            (delta - size).max(0.0)
        }
    };
    axis(point.x, f64::from(tile.x) / scale).hypot(axis(point.y, f64::from(tile.y) / scale))
}

/// A location in Mercator `0..1` units.
#[cfg(test)]
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
#[cfg(test)]
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
    // GL JS's factor grows without bound as the field narrows, so a sensor's view of a few
    // degrees, tilted, would load tiles levels coarser than its screen resolves. The factor
    // holds back the tile count of a view spanning much of the ground toward the horizon; a
    // field narrower than the normal one sees less ground and needs no more restraint.
    let pitch_behavior =
        pitch_tile_loading_behavior(field_of_view_degrees.max(NORMAL_FIELD_OF_VIEW_DEGREES));
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

#[cfg(test)]
mod tests;
