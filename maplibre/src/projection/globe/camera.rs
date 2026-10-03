//! Vertical-perspective globe camera matrices and screen-space conversion.

use cgmath::{
    perspective, Deg, InnerSpace, Matrix3, Matrix4, Point2, Rad, SquareMatrix, Vector3, Vector4,
};
use thiserror::Error;

use super::{
    elevate_surface_point, lat_lon_to_unit_sphere, mercator_to_angular_radians,
    project_tile_coordinates_to_unit_sphere,
};
use crate::{
    coords::{LatLon, TileCoords, EXTENT},
    projection::{body::Body, globe::globe_radius_pixels, renderer_data::ProjectionDataError},
    render::camera::{OPENGL_TO_WGPU_MATRIX, REVERSED_Z},
};

const NEAR_Z: f64 = 0.5;
const MIN_DIRECTION_LENGTH_SQUARED: f64 = 1e-24;

/// Inputs used to create a vertical-perspective globe camera state.
#[derive(Clone, Copy, Debug)]
pub struct GlobeCameraOptions {
    /// Viewport width in logical pixels.
    pub width: f64,
    /// Viewport height in logical pixels.
    pub height: f64,
    /// Vertical field of view in degrees.
    pub field_of_view_degrees: f64,
    /// Geographic map center.
    pub center: LatLon,
    /// Web Mercator world width in pixels at the current zoom.
    pub world_size: f64,
    /// Clockwise bearing in degrees.
    pub bearing_degrees: f64,
    /// Pitch in degrees.
    pub pitch_degrees: f64,
    /// Roll in degrees.
    pub roll_degrees: f64,
    /// Offset of the perspective center from viewport center, in pixels.
    pub center_offset: Point2<f64>,
    /// The body the globe stands for; its radius scales elevations onto the unit sphere.
    pub body: Body,
    /// Height in metres of the point the camera orbits above the center's sea level, as the
    /// terrain is drawn there, exaggeration included; zero orbits the sea-level surface.
    pub target_elevation_meters: f64,
}

/// Screen projection of a globe point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlobePointProjection {
    /// Point in normalized device coordinates.
    pub point: Point2<f64>,
    /// Homogeneous clip-space W, preserving signed camera distance ordering.
    pub signed_distance_from_camera: f64,
    /// Whether the unit-sphere surface point lies behind the horizon.
    pub is_occluded: bool,
}

/// Failure while constructing or querying a globe camera.
#[derive(Clone, Copy, Debug, Error, PartialEq)]
pub enum GlobeCameraError {
    /// Viewport dimensions must be finite and positive.
    #[error("globe viewport must be finite and positive, got {width}x{height}")]
    InvalidViewport {
        /// Invalid viewport width.
        width: f64,
        /// Invalid viewport height.
        height: f64,
    },
    /// Field of view must be finite and strictly between zero and 180 degrees.
    #[error("globe field of view must be between 0 and 180 degrees, got {degrees}")]
    InvalidFieldOfView {
        /// Invalid field of view in degrees.
        degrees: f64,
    },
    /// World size must be finite and positive.
    #[error("globe world size must be finite and positive, got {world_size}")]
    InvalidWorldSize {
        /// Invalid world size in pixels.
        world_size: f64,
    },
    /// A camera angle or center component must be finite.
    #[error("{name} must be finite, got {degrees}")]
    InvalidAngle {
        /// Name of the invalid angle.
        name: &'static str,
        /// Invalid angle in degrees.
        degrees: f64,
    },
    /// Perspective center offset must be finite.
    #[error("globe perspective center offset must be finite, got ({x}, {y})")]
    InvalidCenterOffset {
        /// Invalid horizontal offset.
        x: f64,
        /// Invalid vertical offset.
        y: f64,
    },
    /// The orbit target lies at or below the center of the body.
    #[error("the camera target lies {meters} m above sea level, at or below the body's center")]
    InvalidTargetElevation {
        /// Invalid target elevation in metres.
        meters: f64,
    },
    /// The eye sits on or under the surface its horizon is measured on, where it has none.
    #[error("the eye is {distance} radii from the globe center, on or below the surface")]
    EyeBelowSurface {
        /// Distance of the eye from the globe center in radii.
        distance: f64,
    },
    /// The derived projection matrix cannot be inverted.
    #[error("globe view-projection matrix is not invertible")]
    NonInvertibleViewProjection,
    /// Projection-data geometry is invalid.
    #[error("failed to compute globe projection data")]
    ProjectionData {
        /// Underlying projection-data error.
        #[source]
        source: ProjectionDataError,
    },
}

/// Immutable matrices and derived geometry for one globe camera configuration.
#[derive(Clone, Debug)]
pub struct GlobeCameraState {
    options: GlobeCameraOptions,
    projection: Matrix4<f64>,
    inverse_projection: Matrix4<f64>,
    view: Matrix4<f64>,
    view_projection: Matrix4<f64>,
    inverse_view_projection: Matrix4<f64>,
    camera_position: Vector3<f64>,
    clipping_plane: Vector4<f64>,
    globe_radius_pixels: f64,
    camera_to_center_distance: f64,
    near_z: f64,
    far_z: f64,
    /// Whether the camera is an eye a host placed rather than one derived from map angles.
    external_eye: bool,
}

impl GlobeCameraState {
    /// Builds camera matrices and horizon geometry from validated options.
    pub fn new(options: GlobeCameraOptions) -> Result<Self, GlobeCameraError> {
        validate_options(options)?;
        let camera_to_center_distance = camera_to_center_distance(options);
        let radius = globe_radius_pixels(options.world_size, options.center.latitude);
        let target = target_scale(options)?;
        // The line of sight to the horizon of a sphere scaled by `target` is shorter than the
        // eye's distance to the globe center, and the tallest terrain beyond that horizon adds
        // a small part of a radius, so this far plane holds everything in front of the eye.
        let far_z = camera_to_center_distance + radius * 2.0 * target;
        let projection = projection_matrix(options, far_z);
        let inverse_projection = projection
            .invert()
            .ok_or(GlobeCameraError::NonInvertibleViewProjection)?;
        let view = globe_view_matrix(options, camera_to_center_distance, radius, target);
        let view_projection = projection * view;
        let inverse_view_projection = view_projection
            .invert()
            .ok_or(GlobeCameraError::NonInvertibleViewProjection)?;
        let camera_position = camera_position(options, camera_to_center_distance, radius, target);
        // Below sea level, as in a depression, the eye can sit inside the sea-level sphere,
        // which then hides nothing; the ground it stands over does.
        let clipping_plane = horizon_plane(camera_position, target.min(1.0))?;

        Ok(Self {
            options,
            projection,
            inverse_projection,
            view,
            view_projection,
            inverse_view_projection,
            camera_position,
            clipping_plane,
            globe_radius_pixels: radius,
            camera_to_center_distance,
            near_z: NEAR_Z,
            far_z,
            external_eye: false,
        })
    }

    /// Returns the OpenGL-convention perspective matrix without view transforms.
    pub fn projection(&self) -> Matrix4<f64> {
        self.projection
    }

    /// Returns the inverse perspective matrix in OpenGL clip-space conventions.
    pub fn inverse_projection(&self) -> Matrix4<f64> {
        self.inverse_projection
    }

    /// Returns the globe-to-view transform without perspective projection.
    pub fn view(&self) -> Matrix4<f64> {
        self.view
    }

    /// Returns the OpenGL-convention matrix projecting the unit globe to clip space.
    pub fn view_projection(&self) -> Matrix4<f64> {
        self.view_projection
    }

    /// Returns the matrix in GPU clip conventions: WebGPU depth range with reversed-Z.
    pub fn wgpu_view_projection(&self) -> Matrix4<f64> {
        REVERSED_Z * OPENGL_TO_WGPU_MATRIX * self.view_projection
    }

    /// Returns the inverse OpenGL-convention globe view-projection matrix.
    pub fn inverse_view_projection(&self) -> Matrix4<f64> {
        self.inverse_view_projection
    }

    /// Returns the camera position in unit-globe coordinates.
    pub fn camera_position(&self) -> Vector3<f64> {
        self.camera_position
    }

    /// Whether the camera is an eye a host placed, whose position and height are its own
    /// rather than derived from the map center, pitch and distance.
    pub fn is_external_eye(&self) -> bool {
        self.external_eye
    }

    /// The point the camera orbits, in unit-globe coordinates: the center raised to the
    /// target elevation.
    pub fn target(&self) -> Vector3<f64> {
        let scale = 1.0 + self.options.target_elevation_meters / self.options.body.radius_meters;
        lat_lon_to_unit_sphere(self.options.center) * scale
    }

    /// Returns the geographic center used to orient the globe.
    pub fn center(&self) -> LatLon {
        self.options.center
    }

    /// Returns the vertical field of view in degrees.
    pub fn field_of_view_degrees(&self) -> f64 {
        self.options.field_of_view_degrees
    }

    /// Returns the clockwise map bearing in degrees.
    pub fn bearing_degrees(&self) -> f64 {
        self.options.bearing_degrees
    }

    /// Returns the camera pitch in degrees.
    pub fn pitch_degrees(&self) -> f64 {
        self.options.pitch_degrees
    }

    /// Returns the Mercator world width used to scale the camera.
    pub fn world_size(&self) -> f64 {
        self.options.world_size
    }

    /// Returns the camera distance to the map center in screen pixels.
    pub fn camera_to_center_distance(&self) -> f64 {
        self.camera_to_center_distance
    }

    /// Returns the normalized horizon plane in unit-globe coordinates.
    pub fn clipping_plane(&self) -> Vector4<f64> {
        self.clipping_plane
    }

    /// Returns the globe radius in screen pixels.
    pub fn globe_radius_pixels(&self) -> f64 {
        self.globe_radius_pixels
    }

    /// Returns the near and far clipping distances.
    pub fn depth_range(&self) -> (f64, f64) {
        (self.near_z, self.far_z)
    }

    /// Returns Mercator-to-globe pixel scaling at the map center.
    pub fn pixel_scale(&self) -> f64 {
        1.0 / self.options.center.latitude.to_radians().cos()
    }

    /// Returns circle-radius correction at the map center latitude.
    pub fn circle_radius_correction(&self) -> f64 {
        self.options.center.latitude.to_radians().cos()
    }

    /// Returns text-size correction for a tile-local anchor under globe projection.
    pub fn pitched_text_correction(&self, _x: f64, y: f64, tile: TileCoords) -> f64 {
        let tile_count = 2_f64.powi(i32::from(u8::from(tile.z)));
        let mercator_y = (f64::from(tile.y) + y / EXTENT) / tile_count;
        let (_, latitude) = mercator_to_angular_radians(0.0, mercator_y);
        self.circle_radius_correction() / latitude.cos()
    }

    /// Transforms a map-local light direction into globe axes at the map center.
    pub fn transform_light_direction(&self, direction: Vector3<f64>) -> Option<Vector3<f64>> {
        let sphere = lat_lon_to_unit_sphere(self.options.center);
        let right = Vector3::new(sphere.z, 0.0, -sphere.x);
        if right.magnitude2() <= MIN_DIRECTION_LENGTH_SQUARED {
            return None;
        }
        let right = right.normalize();
        let down = right.cross(sphere).normalize();
        let transformed = right * direction.x + down * direction.y + sphere * direction.z;
        (transformed.magnitude2() > MIN_DIRECTION_LENGTH_SQUARED).then(|| transformed.normalize())
    }

    /// Transforms a globe-world direction into view axes without applying translation.
    pub fn world_direction_to_view(&self, direction: Vector3<f64>) -> Option<Vector3<f64>> {
        let linear = Matrix3::from_cols(
            self.view.x.truncate(),
            self.view.y.truncate(),
            self.view.z.truncate(),
        );
        let transformed = linear * direction;
        (transformed.magnitude2() > MIN_DIRECTION_LENGTH_SQUARED).then(|| transformed.normalize())
    }

    /// Returns the globe center in view coordinates.
    pub fn globe_center_in_view(&self) -> Vector3<f64> {
        let center = self.view * Vector4::unit_w();
        center.truncate() / center.w
    }

    /// Projects a tile-local coordinate onto the globe and then to clip space.
    pub fn project_tile_coordinates(
        &self,
        x: f64,
        y: f64,
        tile: TileCoords,
        elevation_meters: f64,
    ) -> GlobePointProjection {
        let surface =
            project_tile_coordinates_to_unit_sphere(tile.x, tile.y, u8::from(tile.z), x, y);
        self.project_surface_point(
            elevate_surface_point(surface, elevation_meters, self.options.body),
            surface,
        )
    }

    /// The body the globe stands for.
    pub fn body(&self) -> Body {
        self.options.body
    }

    /// Projects a geographic location to viewport pixels.
    pub fn location_to_screen(&self, location: LatLon, elevation_meters: f64) -> Point2<f64> {
        let surface = lat_lon_to_unit_sphere(location);
        let projected = self.project_surface_point(
            elevate_surface_point(surface, elevation_meters, self.options.body),
            surface,
        );
        Point2::new(
            (projected.point.x * 0.5 + 0.5) * self.options.width,
            (-projected.point.y * 0.5 + 0.5) * self.options.height,
        )
    }

    /// Returns whether a geographic location is hidden behind the planet.
    pub fn is_location_occluded(&self, location: LatLon) -> bool {
        !self.is_surface_point_visible(lat_lon_to_unit_sphere(location))
    }

    fn project_surface_point(
        &self,
        position: Vector3<f64>,
        unelevated_surface: Vector3<f64>,
    ) -> GlobePointProjection {
        let clip = self.view_projection * position.extend(1.0);
        GlobePointProjection {
            point: Point2::new(clip.x / clip.w, clip.y / clip.w),
            signed_distance_from_camera: clip.w,
            is_occluded: !self.is_surface_point_visible(unelevated_surface),
        }
    }

    fn is_surface_point_visible(&self, surface: Vector3<f64>) -> bool {
        self.clipping_plane.truncate().dot(surface) + self.clipping_plane.w >= 0.0
    }
}

/// The orbit target's distance from the globe center in radii.
fn target_scale(options: GlobeCameraOptions) -> Result<f64, GlobeCameraError> {
    let scale = 1.0 + options.target_elevation_meters / options.body.radius_meters;
    if scale.is_finite() && scale > 0.0 {
        Ok(scale)
    } else {
        Err(GlobeCameraError::InvalidTargetElevation {
            meters: options.target_elevation_meters,
        })
    }
}

/// The plane through the points where lines from `eye` touch the sphere of `radius` about
/// the globe center: the points `P` of that sphere with `dot(eye, P) = radius²`, normalised so
/// the visible side is positive. On the unit sphere this is `dot(eye, P) = 1`.
pub(super) fn horizon_plane(
    eye: Vector3<f64>,
    radius: f64,
) -> Result<Vector4<f64>, GlobeCameraError> {
    let distance = eye.magnitude();
    if distance.is_nan() || distance <= radius {
        return Err(GlobeCameraError::EyeBelowSurface { distance });
    }
    Ok((eye / distance).extend(-radius * radius / distance))
}

fn camera_to_center_distance(options: GlobeCameraOptions) -> f64 {
    (options.height * 0.5) / (options.field_of_view_degrees.to_radians() * 0.5).tan()
}

fn projection_matrix(options: GlobeCameraOptions, far_z: f64) -> Matrix4<f64> {
    let mut projection = perspective(
        Deg(options.field_of_view_degrees),
        options.width / options.height,
        NEAR_Z,
        far_z,
    );
    projection.z.x = -options.center_offset.x * 2.0 / options.width;
    projection.z.y = options.center_offset.y * 2.0 / options.height;
    projection
}

fn globe_view_matrix(
    options: GlobeCameraOptions,
    camera_distance: f64,
    radius: f64,
    target: f64,
) -> Matrix4<f64> {
    Matrix4::from_translation(Vector3::new(0.0, 0.0, -camera_distance))
        * Matrix4::from_angle_z(Deg(options.roll_degrees))
        * Matrix4::from_angle_x(Deg(-options.pitch_degrees))
        * Matrix4::from_angle_z(Deg(options.bearing_degrees))
        * Matrix4::from_translation(Vector3::new(0.0, 0.0, -radius * target))
        * Matrix4::from_angle_x(Deg(options.center.latitude))
        * Matrix4::from_angle_y(Deg(-options.center.longitude))
        * Matrix4::from_scale(radius)
}

fn camera_position(
    options: GlobeCameraOptions,
    camera_distance: f64,
    radius: f64,
    target: f64,
) -> Vector3<f64> {
    let mut position = Vector3::new(0.0, 0.0, camera_distance / radius);
    position = Matrix3::from_angle_z(Deg(-options.roll_degrees)) * position;
    position = Matrix3::from_angle_x(Deg(options.pitch_degrees)) * position;
    position = Matrix3::from_angle_z(Deg(-options.bearing_degrees)) * position;
    position += Vector3::unit_z() * target;
    position = Matrix3::from_angle_x(Deg(-options.center.latitude)) * position;
    Matrix3::from_angle_y(Rad(options.center.longitude.to_radians())) * position
}

mod external;
mod screen;
mod validation;

use validation::validate_options;

pub use external::ExternalGlobeEye;

#[cfg(test)]
mod tests;
