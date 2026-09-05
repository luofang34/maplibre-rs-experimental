//! External views: a camera pose and projection supplied by the host, for head tracking or a
//! second eye.
//!
//! The host hands over the matrices it renders with. The map derives a camera pose from the
//! view matrix, so center, zoom and pixel scale follow the eye as they do for the map's own
//! gestures, and uses the projection as it is, so an asymmetric per-eye frustum drives both
//! the tile covering and the drawn frame.

use cgmath::{Deg, InnerSpace, Matrix4, Point2, Rad, SquareMatrix, Vector3, Vector4};
use thiserror::Error;

use crate::{
    coords::{LatLon, TILE_SIZE},
    projection::body::Body,
    render::{
        camera::FLIP_Y,
        view_state::{
            pose::{lat_lon_at_mercator, mercator_from_lat_lon, CameraPose},
            ViewState,
        },
    },
};

/// Where the local frame of an [`ExternalView`] is anchored.
#[derive(Clone, Copy, Debug)]
pub struct ExternalAnchor {
    /// Ground position of the frame origin.
    pub position: LatLon,
    /// Altitude of the frame origin in metres above sea level.
    pub altitude_meters: f64,
}

/// View and projection matrices supplied by the host.
#[derive(Clone, Copy, Debug)]
pub struct ExternalView {
    /// Origin of the local frame the view matrix starts from.
    pub anchor: ExternalAnchor,
    /// Local frame to camera space.
    ///
    /// The local frame measures metres east, north and up from the anchor. Camera space
    /// follows the OpenGL convention: x right, y up, the camera looking along negative z.
    pub view: Matrix4<f64>,
    /// Camera space to OpenGL clip space; the map applies its own depth conventions on top.
    pub projection: Matrix4<f64>,
}

/// Why an external view cannot drive the map.
#[derive(Error, Debug, Clone, Copy, PartialEq)]
pub enum ExternalViewError {
    /// The view matrix has no inverse, so it places the camera nowhere.
    #[error("the external view matrix is singular")]
    SingularView,
    /// The view pitches further than the map allows; clamping it would leave the pose and the
    /// host's frustum disagreeing.
    #[error("the external view pitches to {pitch:?}, beyond the limit of {max_pitch:?}")]
    PitchBeyondLimit {
        /// Pitch the view matrix describes.
        pitch: Deg<f64>,
        /// Largest pitch the map accepts.
        max_pitch: Deg<f64>,
    },
}

impl ViewState {
    /// Drives the map from a host-supplied view and projection.
    ///
    /// The view matrix becomes a [`CameraPose`], applied as
    /// [`set_camera_pose`](Self::set_camera_pose) would; a pitch beyond the pitch limit is
    /// refused rather than clamped, since the host's frustum would no longer match the pose.
    /// The projection replaces the map's perspective until
    /// [`clear_external_view`](Self::clear_external_view). The globe camera keeps its own
    /// perspective.
    pub fn set_external_view(&mut self, external: ExternalView) -> Result<(), ExternalViewError> {
        let pose = pose_of(&external, self.body())?;
        let max_pitch: Deg<f64> = self.camera().max_pitch().into();
        if pose.pitch.0 > max_pitch.0 + PITCH_TOLERANCE_DEGREES {
            return Err(ExternalViewError::PitchBeyondLimit {
                pitch: pose.pitch,
                max_pitch,
            });
        }
        self.set_camera_pose(pose);
        self.external_projection = Some(external.projection);
        Ok(())
    }

    /// Returns to the map's own perspective; the pose the external view left stays.
    pub fn clear_external_view(&mut self) {
        self.external_projection = None;
    }

    /// The projection a host supplied, if an external view is in effect.
    pub fn external_projection(&self) -> Option<Matrix4<f64>> {
        self.external_projection
    }

    /// The map's own view in the form a host would supply, anchored at the map center on the
    /// center elevation.
    ///
    /// Feeding it back through [`set_external_view`](Self::set_external_view) reproduces the
    /// frame, which is also how a host learns the frame the map would render on its own.
    pub fn external_view(&self) -> ExternalView {
        let world_size = TILE_SIZE * 2f64.powf(self.zoom().value());
        let center = self.camera().position();
        let anchor = ExternalAnchor {
            position: lat_lon_at_mercator(Point2::new(
                center.x / world_size,
                center.y / world_size,
            )),
            altitude_meters: self.center_elevation(),
        };
        let pixels_per_meter = self.pixels_per_meter();
        let local_to_world =
            Matrix4::from_translation(Vector3::new(center.x, center.y, self.center_elevation()))
                * Matrix4::from_nonuniform_scale(pixels_per_meter, -pixels_per_meter, 1.0);
        ExternalView {
            anchor,
            view: FLIP_Y * self.camera_matrix() * local_to_world,
            projection: self
                .external_projection
                .unwrap_or_else(|| FLIP_Y * self.perspective_matrix() * FLIP_Y),
        }
    }
}

/// Slack on the pitch limit, so a pose that sits exactly on it is not refused by rounding.
const PITCH_TOLERANCE_DEGREES: f64 = 1e-9;

/// The camera pose an external view matrix describes.
fn pose_of(external: &ExternalView, body: Body) -> Result<CameraPose, ExternalViewError> {
    let inverse = external
        .view
        .invert()
        .ok_or(ExternalViewError::SingularView)?;
    let eye = inverse * Vector4::new(0.0, 0.0, 0.0, 1.0);
    if eye.w.abs() <= f64::EPSILON {
        return Err(ExternalViewError::SingularView);
    }
    let eye = eye.truncate() / eye.w;
    let forward = (inverse * Vector4::new(0.0, 0.0, -1.0, 0.0))
        .truncate()
        .normalize();
    let right = (inverse * Vector4::new(1.0, 0.0, 0.0, 0.0))
        .truncate()
        .normalize();

    let pitch = (-forward.z).clamp(-1.0, 1.0).acos();
    // Looking straight down, the forward direction says nothing about the bearing; the right
    // vector, which lies on the ground, still does.
    let bearing = if pitch.sin() > 1e-9 {
        forward.x.atan2(forward.y)
    } else {
        (-right.y).atan2(right.x)
    };
    let level_right = Vector3::new(bearing.cos(), -bearing.sin(), 0.0);
    let level_up = level_right.cross(forward);
    // The map turns the view by minus the roll about its axis, which is where the sign comes
    // from.
    let roll = (-right.dot(level_up)).atan2(right.dot(level_right));

    let anchor = external.anchor;
    let units_per_meter =
        1.0 / (body.circumference_meters() * anchor.position.latitude.to_radians().cos());
    let anchor_mercator = mercator_from_lat_lon(anchor.position);
    Ok(CameraPose {
        position: lat_lon_at_mercator(Point2::new(
            anchor_mercator.x + eye.x * units_per_meter,
            anchor_mercator.y - eye.y * units_per_meter,
        )),
        altitude_meters: anchor.altitude_meters + eye.z,
        bearing: Rad(bearing).into(),
        pitch: Rad(pitch).into(),
        roll: Rad(roll).into(),
    })
}

#[cfg(test)]
mod tests;
