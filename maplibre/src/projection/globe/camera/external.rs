//! A globe camera driven by an eye the host tracks, rather than by center, zoom and angles.
//!
//! The eye arrives in unit-sphere coordinates with its own frustum. The matrices use the same
//! pixel units as a camera built from map angles, so the tile covering, symbol placement and
//! depth read them the way they read any globe camera.

use cgmath::{InnerSpace, Matrix, Matrix3, Matrix4, SquareMatrix, Vector3, Vector4};

use super::{validate_options, GlobeCameraError, GlobeCameraOptions, GlobeCameraState};
use crate::{projection::globe::globe_radius_pixels, render::camera::EyeFrustum};

/// An eye placed in unit-sphere coordinates.
#[derive(Clone, Copy, Debug)]
pub struct ExternalGlobeEye {
    /// Where the eye is; a length of one sits on the surface.
    pub position: Vector3<f64>,
    /// Axes of the eye's space in unit-sphere coordinates, columns x right, y up and z
    /// backwards, as OpenGL orders them.
    pub axes: Matrix3<f64>,
    /// The eye's frustum with clip distances in screen pixels.
    pub frustum: EyeFrustum,
    /// Distance from the eye to the map center in screen pixels, which the zoom-dependent
    /// readers of the camera take as its distance to the surface.
    pub camera_to_center_distance: f64,
}

impl ExternalGlobeEye {
    /// The eye a unit-sphere-to-eye view matrix places; `None` when the matrix is singular.
    pub fn from_view(
        view: Matrix4<f64>,
        frustum: EyeFrustum,
        camera_to_center_distance: f64,
    ) -> Option<Self> {
        let inverse = view.invert()?;
        let position = inverse * Vector4::new(0.0, 0.0, 0.0, 1.0);
        if position.w.abs() <= f64::EPSILON {
            return None;
        }
        let axis =
            |direction: Vector3<f64>| (inverse * direction.extend(0.0)).truncate().normalize();
        Some(Self {
            position: position.truncate() / position.w,
            axes: Matrix3::from_cols(
                axis(Vector3::unit_x()),
                axis(Vector3::unit_y()),
                axis(Vector3::unit_z()),
            ),
            frustum,
            camera_to_center_distance,
        })
    }
}

impl GlobeCameraState {
    /// Builds the camera an external eye sees the globe with.
    ///
    /// The options give the map center, zoom and angles the eye's pose derives to, which
    /// scale the globe to pixels and orient the readers that sort or cull by map angles; the
    /// eye itself sets the matrices.
    pub fn from_external_eye(
        options: GlobeCameraOptions,
        eye: ExternalGlobeEye,
    ) -> Result<Self, GlobeCameraError> {
        validate_options(options)?;
        let distance = eye.position.magnitude();
        if distance.is_nan() || distance <= 1.0 {
            return Err(GlobeCameraError::EyeBelowSurface { distance });
        }
        let radius = globe_radius_pixels(options.world_size, options.center.latitude);
        // The eye's axes are expressed in sphere space, so their transpose takes sphere
        // space into the eye's; the sphere is scaled to pixels first.
        let view = Matrix4::from(eye.axes.transpose())
            * Matrix4::from_translation(-eye.position * radius)
            * Matrix4::from_scale(radius);
        let far_z = if eye.frustum.far.is_finite() {
            eye.frustum.far
        } else {
            eye.camera_to_center_distance + radius * 2.0
        };
        let frustum = EyeFrustum {
            far: far_z,
            ..eye.frustum
        };
        let projection = frustum.projection();
        let inverse_projection = projection
            .invert()
            .ok_or(GlobeCameraError::NonInvertibleViewProjection)?;
        let view_projection = projection * view;
        let inverse_view_projection = view_projection
            .invert()
            .ok_or(GlobeCameraError::NonInvertibleViewProjection)?;
        Ok(Self {
            options,
            projection,
            inverse_projection,
            view,
            view_projection,
            inverse_view_projection,
            camera_position: eye.position,
            // The horizon is where the surface is tangent to a line through the eye.
            clipping_plane: (eye.position / distance).extend(-1.0 / distance),
            globe_radius_pixels: radius,
            camera_to_center_distance: eye.camera_to_center_distance,
            near_z: frustum.near,
            far_z,
        })
    }
}

#[cfg(test)]
mod tests;
