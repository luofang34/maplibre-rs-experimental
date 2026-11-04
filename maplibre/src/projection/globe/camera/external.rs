//! A globe camera driven by an eye the host tracks, rather than by center, zoom and angles.
//!
//! The eye arrives in unit-sphere coordinates with its own frustum. The matrices use the same
//! pixel units as a camera built from map angles, so the tile covering, symbol placement and
//! depth read them the way they read any globe camera.

use cgmath::{InnerSpace, Matrix, Matrix3, Matrix4, SquareMatrix, Vector3, Vector4};

use super::{validate_options, GlobeCameraError, GlobeCameraOptions, GlobeCameraState};
use crate::render::camera::EyeFrustum;

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
        // The eye is where the host placed it; it orbits nothing, so no elevation raises the
        // point the camera reports as its target.
        let options = GlobeCameraOptions {
            target_elevation_meters: 0.0,
            ..options
        };
        let lowest_radius = options
            .body
            .unit_radius_at(options.body.lowest_ground_meters)
            .min(1.0);
        let distance = eye.position.magnitude();
        let clearance = 0.25 / options.body.radius_meters;
        let horizon_radius = if distance > lowest_radius {
            (distance - clearance).clamp(lowest_radius, 1.0)
        } else {
            lowest_radius
        };
        let clipping_plane = super::horizon_plane(eye.position, horizon_radius)?;
        let radius = super::radius_pixels(options);
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
            clipping_plane,
            globe_radius_pixels: radius,
            camera_to_center_distance: eye.camera_to_center_distance,
            near_z: frustum.near,
            far_z,
            external_eye: true,
        })
    }
}

const GAZE_REACH_HEIGHTS: f64 = 2.0;

pub(super) fn focus_unit_sphere(
    eye: Vector3<f64>,
    view: &Matrix4<f64>,
    external_eye: bool,
) -> Vector3<f64> {
    let forward_raw = -Vector3::new(view.x.z, view.y.z, view.z.z);
    let len2 = forward_raw.magnitude2();
    if len2 <= super::MIN_DIRECTION_LENGTH_SQUARED || eye.magnitude2() <= 1.0 {
        return if eye.magnitude2() > super::MIN_DIRECTION_LENGTH_SQUARED {
            eye.normalize()
        } else {
            Vector3::unit_z()
        };
    }
    let forward = forward_raw / len2.sqrt();
    if external_eye {
        return external_focus_unit_sphere(eye, forward);
    }
    let b = eye.dot(forward);
    let disc = b * b - (eye.magnitude2() - 1.0);
    let t = if b < 0.0 && disc >= 0.0 {
        -b - disc.sqrt()
    } else {
        (-b).max(0.0)
    };
    (eye + forward * t).normalize()
}

fn external_focus_unit_sphere(origin: Vector3<f64>, direction: Vector3<f64>) -> Vector3<f64> {
    let distance = origin.magnitude();
    if distance <= 1.0 {
        return if distance > super::MIN_DIRECTION_LENGTH_SQUARED.sqrt() {
            origin / distance
        } else {
            Vector3::unit_z()
        };
    }
    let beneath = origin / distance;
    let along = origin.dot(direction);
    let discriminant = along * along - (distance * distance - 1.0);
    let reach = (GAZE_REACH_HEIGHTS * (distance - 1.0) / distance).atan();
    if discriminant >= 0.0 {
        let t = -along - discriminant.sqrt();
        if t >= 0.0 {
            let center = (origin + direction * t).normalize();
            let cosine = beneath.dot(center).clamp(-1.0, 1.0);
            if cosine.acos() <= reach {
                return center;
            }
            let tangent = center - beneath * cosine;
            if tangent.magnitude2() <= super::MIN_DIRECTION_LENGTH_SQUARED {
                return beneath;
            }
            return beneath * reach.cos() + tangent.normalize() * reach.sin();
        }
    }
    let up_dot = beneath.dot(direction);
    let tangent = direction - beneath * up_dot;
    let tangent_len = tangent.magnitude();
    if tangent_len <= super::MIN_DIRECTION_LENGTH_SQUARED.sqrt() {
        return beneath;
    }
    let limb_angle = (1.0 / distance).clamp(-1.0, 1.0).acos();
    let base_angle = reach.min(limb_angle);
    let angle = if up_dot > 0.0 {
        base_angle * tangent_len.clamp(0.0, 1.0)
    } else {
        base_angle
    };
    beneath * angle.cos() + (tangent / tangent_len) * angle.sin()
}

#[cfg(test)]
mod tests;
