//! How the camera navigates the globe: as the map's own camera, or free to cross the poles.
//!
//! Constrained navigation describes the camera by a center, zoom, bearing, pitch and roll,
//! the center in the Mercator world, which ends short of the poles; it turns freely about its
//! center and does not hold north up. Free-globe navigation
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

/// Sine of the tilt below which the camera counts as looking straight down or up, its roll
/// folded into its bearing; well above rounding, well below a pixel's turn.
const VERTICAL_TILT: f64 = 1e-8;

/// How the camera navigates the globe.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NavigationMode {
    /// The map's own camera: a center, zoom, bearing, pitch and roll, the center kept within the
    /// Mercator world. It turns to any bearing and does not hold north up.
    #[default]
    #[serde(alias = "NorthLocked")]
    Constrained,
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
    /// Constrained navigation cannot show a target this far towards a pole.
    #[error("the camera looks at latitude {latitude}, past the Mercator world constrained navigation shows")]
    BeyondMercatorWorld {
        /// The latitude of the free camera's target.
        latitude: f64,
    },
    /// The pose is not finite, its target or orientation has no direction, its distance is not
    /// positive, or no globe camera can be drawn from it, as when its eye ends inside the body.
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
    /// to constrained navigation at the nearest view it can show.
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
    /// longitude defines, which the view matrix rebuilds exactly. Looking straight down or
    /// straight up, roll and bearing turn about the same axis and the turn is all bearing:
    /// `Rz(roll) Rx(0) Rz(bearing)` turns by `roll + bearing`, `Rz(roll) Rx(-PI) Rz(bearing)`
    /// is `Rz(roll - bearing)` with y and z flipped.
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
        let (bearing, roll) = if tilt > VERTICAL_TILT {
            ((-m(2, 0)).atan2(-m(2, 1)), (-m(0, 2)).atan2(m(1, 2)))
        } else if m(2, 2) > 0.0 {
            (m(1, 0).atan2(m(0, 0)), 0.0)
        } else {
            (-(m(1, 0).atan2(m(0, 0))), 0.0)
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
            NavigationMode::Constrained
        }
    }

    /// Why free navigation last ended without the host asking, until the host sets a mode.
    pub fn navigation_limit(&self) -> Option<NavigationLimit> {
        self.navigation_limit
    }

    /// Switches navigation, keeping the camera where it is. Constrained navigation cannot show
    /// a target past the Mercator world's last latitude, so a free camera looking there is
    /// refused rather than moved; the host brings it back first.
    pub fn set_navigation_mode(
        &mut self,
        mode: NavigationMode,
        projection: &ProjectionType,
    ) -> Result<(), NavigationError> {
        self.navigation_limit = None;
        match mode {
            NavigationMode::Constrained => {
                if let Some(view) = self.pose_view() {
                    if view.center.latitude.abs() > scale::MERCATOR_LATITUDE_LIMIT {
                        return Err(NavigationError::BeyondMercatorWorld {
                            latitude: view.center.latitude,
                        });
                    }
                }
                self.free_globe = None;
                Ok(())
            }
            NavigationMode::FreeGlobe if self.free_globe.is_some() => Ok(()),
            NavigationMode::FreeGlobe => {
                supports_free_navigation(projection)?;
                if self.has_external_view() {
                    return Err(NavigationError::ExternalView);
                }
                let pose = self.constrained_pose();
                self.set_globe_pose(pose)
            }
        }
    }

    /// The free-globe camera's pose; `None` while navigation is constrained.
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

    /// Places the free-globe camera at `pose`, entering free navigation, for the projection the
    /// map draws with, which must allow it; a stored pose is restored this way.
    pub fn restore_globe_pose(
        &mut self,
        pose: GlobePose,
        projection: &ProjectionType,
    ) -> Result<(), NavigationError> {
        supports_free_navigation(projection)?;
        self.navigation_limit = None;
        self.set_globe_pose(pose)
    }

    /// Places the free-globe camera at `pose`, entering free navigation; the projection must
    /// already allow it, as [`restore_globe_pose`](Self::restore_globe_pose) checks.
    pub fn set_globe_pose(&mut self, pose: GlobePose) -> Result<(), NavigationError> {
        if self.has_external_view() {
            return Err(NavigationError::ExternalView);
        }
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
        let free = FreeGlobe {
            target: target.normalize(),
            orientation: orientation.normalize(),
            distance_meters: pose.distance_meters,
        };
        // Off the ground the pose's target height is where the center rests from now on. On
        // it the terrain under the target decides the height from the next frame, and the
        // altitude the center rests at once off the ground again, the style's centerAltitude
        // or the host's, stays as it was.
        let placed = self.place_free_globe(free, |view| {
            if view.center_held_by_terrain() {
                view.set_center_elevation(pose.target_elevation_meters);
            } else {
                view.set_center_altitude(pose.target_elevation_meters);
            }
        });
        if placed {
            Ok(())
        } else {
            Err(NavigationError::InvalidPose { pose })
        }
    }

    /// Puts the free-globe camera at `free`, with `height` applied, if the globe camera can be
    /// drawn from it, as one past 90 degrees of pitch can when it looks up at a raised target,
    /// but not one whose eye ends inside the body; otherwise the view stays as it was.
    fn place_free_globe(&mut self, free: FreeGlobe, height: impl FnOnce(&mut Self)) -> bool {
        let before = self.clone();
        self.free_globe = Some(free);
        height(self);
        self.sync_flat_camera();
        self.keep_if_drawable(before)
    }

    /// Turns the free-globe camera of `from` about its target: its bearing by `bearing` and its
    /// pitch by `pitch`, the pitch kept between straight down and the camera's limit, or the
    /// pose's own pitch where a host placed it past the limit, so a turn never jumps. The
    /// target, distance and roll stay, and the center's height is left to whatever holds it. A
    /// drag hands the pose it started from with its whole turn so far.
    pub fn orbit_globe_pose(
        &mut self,
        from: GlobePose,
        bearing: Rad<f64>,
        pitch: Rad<f64>,
    ) -> Result<(), NavigationError> {
        let start = FreeGlobe {
            target: Vector3::from(from.target).normalize(),
            orientation: Quaternion::new(
                from.orientation[0],
                from.orientation[1],
                from.orientation[2],
                from.orientation[3],
            )
            .normalize(),
            distance_meters: from.distance_meters,
        };
        let (center, start_bearing, start_pitch, roll) = start.decompose();
        let limit = self
            .camera()
            .max_pitch()
            .0
            .to_degrees()
            .max(start_pitch)
            .min(180.0);
        let turned = view_rotation(
            center,
            start_bearing + bearing.0.to_degrees(),
            (start_pitch + pitch.0.to_degrees()).clamp(0.0, limit),
            roll,
        );
        if self.has_external_view() {
            return Err(NavigationError::ExternalView);
        }
        let turned = FreeGlobe {
            orientation: Quaternion::from(turned).normalize(),
            ..start
        };
        if self.place_free_globe(turned, |_| {}) {
            Ok(())
        } else {
            Err(NavigationError::InvalidPose {
                pose: GlobePose {
                    orientation: [
                        turned.orientation.s,
                        turned.orientation.v.x,
                        turned.orientation.v.y,
                        turned.orientation.v.z,
                    ],
                    ..from
                },
            })
        }
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

    /// The pose of the current constrained view.
    fn constrained_pose(&self) -> GlobePose {
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
