//! How the camera navigates the globe: locked to north, or free to turn and to cross the poles.
//!
//! North-locked navigation describes the camera by a center, zoom, bearing, pitch and roll,
//! the center in the Mercator world, which ends short of the poles. Free-globe navigation
//! describes it by a [`GlobePose`]: the point it looks at as a direction from the body's
//! center, its orientation, and its distance in metres. None of these are style fields; a host
//! sets them at runtime and may store a pose in its own documents.
//!
//! Free navigation is offered for the pure `vertical-perspective` projection only, the one
//! projection that draws the globe at every zoom; the globe preset and projection expressions
//! turn into the flat map, which has no pole to cross. The camera is drawn from the pose, and
//! the map's center, zoom, bearing, pitch and roll follow it for the readers of the flat
//! camera: the center stops at the Mercator world's last latitude, and the zoom is the style
//! zoom [`scale::style_zoom`] gives the pose's physical scale.

use cgmath::{InnerSpace, Matrix, Matrix3, Point2, Quaternion, Rad, Vector3};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::ViewState;
use crate::{
    coords::{LatLon, Zoom, TILE_SIZE},
    projection::{
        globe::{lat_lon_to_unit_sphere, scale},
        ProjectionType,
    },
};

/// How the camera navigates the globe.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NavigationMode {
    /// A center, zoom, bearing, pitch and roll, the center kept within the Mercator world.
    #[default]
    NorthLocked,
    /// A pose that may look at any point and turn any way, across the poles included.
    FreeGlobe,
}

/// Where a free-globe camera is and where it looks, in the body's frame.
///
/// The body's frame has y towards the north pole and z towards longitude zero on the equator,
/// the frame the globe is drawn in.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GlobePose {
    /// Direction from the body's center to the point the camera looks at.
    pub target: [f64; 3],
    /// Height of that point above the mean radius in drawn metres.
    pub target_elevation_meters: f64,
    /// Rotation from the body's frame into the camera's, as `[w, x, y, z]`; the camera looks
    /// along its negative z with y up, as OpenGL orders eye space.
    pub orientation: [f64; 4],
    /// Distance from the eye to the target in metres.
    pub distance_meters: f64,
}

/// Why navigation cannot be set as asked.
#[derive(Clone, Debug, Error, PartialEq)]
pub enum NavigationError {
    /// Free navigation needs the pure vertical-perspective projection.
    #[error("free-globe navigation needs the vertical-perspective projection, not {projection}")]
    ProjectionNotSupported {
        /// The projection in use.
        projection: String,
    },
    /// A host's eye drives the camera.
    #[error("a host's eye drives the camera")]
    ExternalView,
    /// The pose is not finite, its target or orientation has no direction, or its distance is
    /// not positive.
    #[error("invalid globe pose {pose:?}")]
    InvalidPose {
        /// The rejected pose.
        pose: GlobePose,
    },
}

/// Why free navigation ended without the host asking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigationLimit {
    /// The style's projection stopped being pure vertical perspective, so the camera returned
    /// to north-locked navigation at the nearest view it can show.
    ProjectionChanged,
}

/// The free-globe camera state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct FreeGlobe {
    target: Vector3<f64>,
    /// Rotation from the body's frame into the camera's.
    orientation: Quaternion<f64>,
    distance_meters: f64,
}

/// The parameters a globe camera is built from, decomposed from a pose.
pub(crate) struct PoseView {
    pub center: LatLon,
    pub bearing_degrees: f64,
    pub pitch_degrees: f64,
    pub roll_degrees: f64,
    pub radius_pixels: f64,
    pub style_zoom: f64,
}

/// The rotation from the body's frame into the camera's for a center, bearing, pitch and roll,
/// as the globe camera's view matrix composes it.
fn view_rotation(center: LatLon, bearing: f64, pitch: f64, roll: f64) -> Matrix3<f64> {
    Matrix3::from_angle_z(Rad(roll.to_radians()))
        * Matrix3::from_angle_x(Rad(-pitch.to_radians()))
        * Matrix3::from_angle_z(Rad(bearing.to_radians()))
        * frame_at(center)
}

/// The rotation turning the body so `center` faces the camera, north up.
fn frame_at(center: LatLon) -> Matrix3<f64> {
    Matrix3::from_angle_x(Rad(center.latitude.to_radians()))
        * Matrix3::from_angle_y(Rad(-center.longitude.to_radians()))
}

impl FreeGlobe {
    /// Decomposes the pose into a center and angles. Any longitude serves at a pole, so the
    /// one `atan2` gives there is taken, and bearing and roll are measured in the frame that
    /// longitude defines, which the view matrix rebuilds exactly. Looking straight down, roll
    /// and bearing turn about the same axis and the turn is all bearing.
    fn decompose(&self) -> (LatLon, f64, f64, f64) {
        let target = self.target;
        let center = LatLon::new(
            target.y.clamp(-1.0, 1.0).asin().to_degrees(),
            target.x.atan2(target.z).to_degrees(),
        );
        let rotation = Matrix3::from(self.orientation) * frame_at(center).transpose();
        // rotation = Rz(roll) Rx(-pitch) Rz(bearing); cgmath indexes [column][row].
        let m = |row: usize, column: usize| rotation[column][row];
        let tilt = m(2, 0).hypot(m(2, 1));
        let pitch = tilt.atan2(m(2, 2));
        let (bearing, roll) = if tilt > 1e-12 {
            ((-m(2, 0)).atan2(-m(2, 1)), (-m(0, 2)).atan2(m(1, 2)))
        } else {
            (m(1, 0).atan2(m(0, 0)), 0.0)
        };
        (
            center,
            bearing.to_degrees(),
            pitch.to_degrees(),
            roll.to_degrees(),
        )
    }
}

impl ViewState {
    /// How the camera navigates the globe.
    pub fn navigation_mode(&self) -> NavigationMode {
        if self.free_globe.is_some() {
            NavigationMode::FreeGlobe
        } else {
            NavigationMode::NorthLocked
        }
    }

    /// Why free navigation last ended without the host asking, until the host sets a mode.
    pub fn navigation_limit(&self) -> Option<NavigationLimit> {
        self.navigation_limit
    }

    /// Switches navigation, keeping the camera where it is: free navigation starts from the
    /// current view, and north-locked navigation from the nearest view it can show.
    pub fn set_navigation_mode(
        &mut self,
        mode: NavigationMode,
        projection: &ProjectionType,
    ) -> Result<(), NavigationError> {
        self.navigation_limit = None;
        match mode {
            NavigationMode::NorthLocked => {
                self.free_globe = None;
                Ok(())
            }
            NavigationMode::FreeGlobe if self.free_globe.is_some() => Ok(()),
            NavigationMode::FreeGlobe => {
                supports_free_navigation(projection)?;
                if self.has_external_view() {
                    return Err(NavigationError::ExternalView);
                }
                let pose = self.north_locked_pose();
                self.set_globe_pose(pose)
            }
        }
    }

    /// The free-globe camera's pose; `None` while navigation is north-locked.
    pub fn globe_pose(&self) -> Option<GlobePose> {
        self.free_globe.map(|free| GlobePose {
            target: free.target.into(),
            target_elevation_meters: self.center_elevation(),
            orientation: [
                free.orientation.s,
                free.orientation.v.x,
                free.orientation.v.y,
                free.orientation.v.z,
            ],
            distance_meters: free.distance_meters,
        })
    }

    /// Places the free-globe camera at `pose`, entering free navigation; the projection must
    /// already allow it, as [`set_navigation_mode`](Self::set_navigation_mode) checks.
    pub fn set_globe_pose(&mut self, pose: GlobePose) -> Result<(), NavigationError> {
        let target = Vector3::from(pose.target);
        let orientation = Quaternion::new(
            pose.orientation[0],
            pose.orientation[1],
            pose.orientation[2],
            pose.orientation[3],
        );
        let finite = pose
            .target
            .iter()
            .chain(&pose.orientation)
            .chain([&pose.distance_meters, &pose.target_elevation_meters])
            .all(|value| value.is_finite());
        if !finite
            || target.magnitude2() < 1e-24
            || orientation.magnitude2() < 1e-24
            || pose.distance_meters <= 0.0
        {
            return Err(NavigationError::InvalidPose { pose });
        }
        self.free_globe = Some(FreeGlobe {
            target: target.normalize(),
            orientation: orientation.normalize(),
            distance_meters: pose.distance_meters,
        });
        self.set_center_elevation(pose.target_elevation_meters);
        self.sync_flat_camera();
        Ok(())
    }

    /// Ends free navigation when the projection no longer allows it, recording why.
    pub(crate) fn enforce_navigation(&mut self, projection: &ProjectionType) {
        if self.free_globe.is_some() && supports_free_navigation(projection).is_err() {
            self.free_globe = None;
            self.navigation_limit = Some(NavigationLimit::ProjectionChanged);
        }
    }

    /// The center, angles, radius and style zoom of the free-globe camera, while there is one.
    pub(crate) fn pose_view(&self) -> Option<PoseView> {
        let free = self.free_globe?;
        let (center, bearing, pitch, roll) = free.decompose();
        let distance = free.distance_meters / self.body().radius_meters;
        let radius_pixels = self.globe_camera_to_center_distance() / distance;
        Some(PoseView {
            center,
            bearing_degrees: bearing,
            pitch_degrees: pitch,
            roll_degrees: roll,
            radius_pixels,
            style_zoom: scale::style_zoom(radius_pixels, center.latitude),
        })
    }

    /// Turns the whole free-globe camera, eye and target, about the body's center.
    pub(crate) fn turn_free_globe(&mut self, turn: Quaternion<f64>) {
        if let Some(free) = &mut self.free_globe {
            free.target = (turn * free.target).normalize();
            free.orientation = (free.orientation * turn.conjugate()).normalize();
        }
        self.sync_flat_camera();
    }

    /// Scales the free-globe camera's distance to its target by `factor`.
    pub(crate) fn scale_free_globe_distance(&mut self, factor: f64) {
        if let Some(free) = &mut self.free_globe {
            if factor.is_finite() && factor > 0.0 {
                free.distance_meters *= factor;
            }
        }
        self.sync_flat_camera();
    }

    /// The camera's distance to its target in pixels, as the globe camera measures it.
    fn globe_camera_to_center_distance(&self) -> f64 {
        self.height() * 0.5 / (self.field_of_view().0 * 0.5).tan()
    }

    /// The pose of the current north-locked view.
    fn north_locked_pose(&self) -> GlobePose {
        let world_size = TILE_SIZE * 2_f64.powf(self.zoom().value());
        let position = self.camera().position();
        let center = crate::render::projection::mercator_world_to_lat_lon(
            position.x, position.y, world_size,
        );
        let rotation = view_rotation(
            center,
            self.camera().get_bearing().0.to_degrees(),
            self.camera().get_pitch().0.to_degrees(),
            self.camera().get_roll().0.to_degrees(),
        );
        let radius = crate::projection::globe::globe_radius_pixels(world_size, center.latitude);
        let orientation = Quaternion::from(rotation);
        GlobePose {
            target: lat_lon_to_unit_sphere(center).into(),
            target_elevation_meters: self.center_elevation(),
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

    /// Points the flat camera's center, zoom and angles at what the free-globe camera shows,
    /// the center held within the Mercator world.
    fn sync_flat_camera(&mut self) {
        let Some(view) = self.pose_view() else {
            return;
        };
        let zoom = Zoom::new(view.style_zoom);
        let world_size = TILE_SIZE * 2_f64.powf(view.style_zoom);
        let latitude = view.center.latitude.clamp(
            -scale::MERCATOR_LATITUDE_LIMIT,
            scale::MERCATOR_LATITUDE_LIMIT,
        );
        let mercator = crate::terrain::sightline::lat_lon_to_mercator(LatLon::new(
            latitude,
            view.center.longitude,
        ));
        self.update_zoom(zoom);
        let camera = self.camera_mut();
        camera.move_to(Point2::new(
            mercator.x * world_size,
            mercator.y * world_size,
        ));
        camera.set_bearing(Rad(view.bearing_degrees.to_radians()));
        camera.set_pitch(Rad(view.pitch_degrees.to_radians()));
        camera.set_roll(Rad(view.roll_degrees.to_radians()));
    }
}

/// Whether `projection` lets the camera navigate freely: pure vertical perspective only.
fn supports_free_navigation(projection: &ProjectionType) -> Result<(), NavigationError> {
    match projection {
        ProjectionType::VerticalPerspective => Ok(()),
        other => Err(NavigationError::ProjectionNotSupported {
            projection: format!("{other:?}"),
        }),
    }
}

/// The rotation turning direction `from` onto direction `to` the short way.
pub(crate) fn turn_between(from: Vector3<f64>, to: Vector3<f64>) -> Quaternion<f64> {
    Quaternion::from_arc(from.normalize(), to.normalize(), None)
}

mod gestures;

#[cfg(test)]
mod tests;
