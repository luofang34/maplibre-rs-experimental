//! External views: an eye the host places, for head tracking, a stereo pair, or a globe the
//! host set down in its own world.
//!
//! The host hands over the view matrix it renders with and the eye's frustum. The map derives
//! its center, zoom and angles from the view matrix, so tile covering and pixel scale follow
//! the eye as they do for the map's own gestures, and draws with matrices built from the eye
//! itself, in its own units, so the frame matches the host's frustum whatever way the eye
//! turns.

use cgmath::{InnerSpace, Matrix4, Point2, Rad, SquareMatrix, Vector3, Vector4};

pub use crate::projection::camera_math::{ExternalAnchor, ExternalView, ExternalViewError};
use crate::{
    coords::TILE_SIZE,
    projection::{body::Body, globe::camera::ExternalGlobeEye, ProjectionType},
    render::{
        camera::{EyeFrustum, FLIP_Y},
        view_state::{
            bounds::{lat_lon_at_mercator, mercator_from_lat_lon},
            external::sphere::SphereEye,
            pose::CameraPose,
            ViewState,
        },
    },
};

mod spatial;
mod sphere;
pub(crate) use spatial::EyeFogParams;

/// What the view state keeps of an external view for the frame.
#[derive(Clone, Copy, Debug)]
pub(super) struct ExternalEye {
    /// Origin of the local frame.
    anchor: ExternalAnchor,
    /// Local frame to eye space, as the host supplied it.
    eye_from_local: Matrix4<f64>,
    /// Eye space units per local-frame metre.
    scale: f64,
    /// The eye's frustum with clip distances in local-frame metres.
    frustum: EyeFrustum,
    /// The eye in unit-sphere coordinates, for the globe camera.
    sphere: SphereEye,
    /// Pixel focal length of the host frustum, unaffected by request overscan.
    lod_focal_pixels: f64,
    /// Physical scale used for cartographic styling, independent of gaze.
    zoom: f64,
    /// How much that zoom moved since the previous eye: a flight or a hand zoom in progress.
    zoom_rate: f64,
}

/// Zoom change between consecutive eyes below which the eye counts as settled. A six second
/// flight across seven zoom levels moves about 0.013 per frame at ninety frames a second.
const SETTLED_ZOOM_RATE: f64 = 0.002;

impl ViewState {
    /// Drives the map from a host-supplied eye.
    ///
    /// The view matrix becomes the map center, zoom and angles. Where the globe is drawn, the
    /// center is the point the eye's view axis meets on the sphere; on the flat map the view
    /// becomes a [`CameraPose`], applied as [`set_camera_pose`](Self::set_camera_pose)
    /// would. Those only steer the tile covering and the pixel scale: the frame is drawn with
    /// matrices built from the eye itself, so a pitch beyond the map's limit, which the pose
    /// clamps, still renders what the eye sees. The eye replaces the map's own camera and
    /// perspective until [`clear_external_view`](Self::clear_external_view).
    pub fn set_external_view(
        &mut self,
        external: ExternalView,
        projection: &ProjectionType,
    ) -> Result<(), ExternalViewError> {
        let frustum = external.frustum;
        let finite = [
            frustum.left,
            frustum.right,
            frustum.top,
            frustum.bottom,
            frustum.near,
            frustum.far,
        ]
        .iter()
        .all(|value| value.is_finite());
        if !finite
            || frustum.left + frustum.right <= 0.0
            || frustum.top + frustum.bottom <= 0.0
            || frustum.near <= 0.0
            || frustum.far <= frustum.near
        {
            return Err(ExternalViewError::InvalidFrustum { frustum });
        }
        let frame = EyeFrame::of(&external.view)?;
        let body = self.body();
        let sphere = SphereEye::place(&frame, external.anchor, body);
        let on_sphere = sphere.pose(body);
        let zoom = self.zoom_for_center_distance(on_sphere.distance_meters, on_sphere.center);
        if projection.uses_globe_rendering(zoom.value()) {
            self.set_center_and_angles(
                on_sphere.center,
                zoom,
                on_sphere.bearing,
                on_sphere.pitch,
                on_sphere.roll,
            );
        } else {
            self.set_eye_pose(
                flat_pose_of(&frame, external.anchor, body),
                external.anchor.altitude_meters,
            );
        }
        let nadir = crate::projection::globe::unit_sphere_to_lat_lon(sphere.position.normalize());
        let height = ((sphere.position.magnitude() - 1.0) * body.radius_meters).max(1.0);
        let zoom = self.zoom_for_center_distance(height, nadir).value();
        let zoom_rate = self
            .external_eye
            .map_or(0.0, |previous| (zoom - previous.zoom).abs());
        self.external_eye = Some(ExternalEye {
            anchor: external.anchor,
            eye_from_local: external.view,
            scale: frame.scale,
            frustum: external.frustum.scaled(1.0 / frame.scale),
            sphere,
            lod_focal_pixels: self.height / (frustum.top + frustum.bottom),
            zoom,
            zoom_rate,
        });
        Ok(())
    }

    /// Whether a host-supplied eye drives the map.
    pub fn has_external_view(&self) -> bool {
        self.external_eye.is_some()
    }

    /// Whether the eye's zoom has come to rest; the map's own camera always counts as
    /// settled. While the zoom moves, every level passed would get requests for ground that
    /// is never looked at.
    pub fn eye_settled(&self) -> bool {
        self.external_eye
            .is_none_or(|eye| eye.zoom_rate <= SETTLED_ZOOM_RATE)
    }

    /// Returns to the map's own perspective; the pose the external view left stays.
    pub fn clear_external_view(&mut self) {
        self.external_eye = None;
    }

    /// The projection an external eye gives the map, in the map's camera units, if an
    /// external view is in effect.
    pub fn external_projection(&self) -> Option<Matrix4<f64>> {
        self.external_frustum().map(|frustum| frustum.projection())
    }

    /// The external eye's frustum with clip distances in world pixels at the anchor.
    pub(super) fn external_frustum(&self) -> Option<EyeFrustum> {
        self.external_eye
            .map(|eye| eye.frustum.scaled(self.anchor_pixels_per_meter(eye.anchor)))
    }

    /// World pixels per metre at a point, the Mercator scale of its latitude.
    fn anchor_pixels_per_meter(&self, anchor: ExternalAnchor) -> f64 {
        let world_size = TILE_SIZE * 2f64.powf(self.zoom().value());
        world_size
            / self
                .body()
                .circumference_at_latitude(anchor.position.latitude)
    }

    /// World space, x and y in pixels and z in metres, to the map's camera space, built from
    /// the external eye rather than the map's angles; `None` without an external view.
    ///
    /// The camera space is in world pixels at the anchor, with y down as the map's own camera
    /// space has it, so the same projection conventions apply on top.
    pub(crate) fn external_camera_matrix(&self) -> Option<Matrix4<f64>> {
        let eye = self.external_eye?;
        let world_size = TILE_SIZE * 2f64.powf(self.zoom().value());
        let pixels_per_meter = self.anchor_pixels_per_meter(eye.anchor);
        let anchor = mercator_from_lat_lon(eye.anchor.position);
        let local_from_world =
            Matrix4::from_nonuniform_scale(1.0 / pixels_per_meter, -1.0 / pixels_per_meter, 1.0)
                * Matrix4::from_translation(Vector3::new(
                    -anchor.x * world_size,
                    -anchor.y * world_size,
                    -eye.anchor.altitude_meters,
                ));
        Some(
            FLIP_Y
                * Matrix4::from_scale(pixels_per_meter / eye.scale)
                * eye.eye_from_local
                * local_from_world,
        )
    }

    /// The view state with the external eye's frustum widened by `factor` on every side, so
    /// a tile covering taken from it reaches beyond the frame; unchanged without an external
    /// eye or with a factor of one.
    pub fn overscanned(&self, factor: f64) -> Option<Self> {
        let eye = self.external_eye?;
        if factor == 1.0 {
            return None;
        }
        let mut widened = self.clone();
        widened.external_eye = Some(ExternalEye {
            frustum: EyeFrustum {
                left: eye.frustum.left * factor,
                right: eye.frustum.right * factor,
                top: eye.frustum.top * factor,
                bottom: eye.frustum.bottom * factor,
                ..eye.frustum
            },
            ..eye
        });
        Some(widened)
    }

    /// Fog distances for an external eye, in world pixels at the anchor: fixed by the eye's
    /// height above the anchor's ground rather than by where it looks, so the haze on a
    /// distant ridge does not change as the head turns. The far distance is the geometric horizon of the
    /// body seen from that height; the fog starts a share of the way there.
    pub(super) fn eye_fog_depth_range(&self) -> Option<(f64, f64)> {
        let eye = self.external_eye?;
        let position = eye.eye_from_local.invert()? * Vector4::new(0.0, 0.0, 0.0, 1.0);
        let height = (position.z / position.w).max(MIN_FOG_HEIGHT_METERS);
        let radius = self.body().circumference_meters() / std::f64::consts::TAU;
        let far = (2.0 * radius * height)
            .sqrt()
            .clamp(MIN_FOG_FAR_METERS, MAX_FOG_FAR_METERS);
        let pixels_per_meter = self.anchor_pixels_per_meter(eye.anchor);
        Some((
            far / FOG_FAR_TO_NEAR * pixels_per_meter,
            far * pixels_per_meter,
        ))
    }

    /// The view state of an eye at the same place looking straight down over a frame that
    /// reaches `reach` times its height to every side; `None` without an external eye. A tile
    /// covering taken from it holds what surrounds the eye, so a turn of the head finds the
    /// tiles loaded.
    pub fn surround(&self, reach: f64, projection: &ProjectionType) -> Option<Self> {
        let eye = self.external_eye?;
        if !self.eye_settled() {
            return None;
        }
        let local_from_eye = eye.eye_from_local.invert()?;
        let position = local_from_eye * Vector4::new(0.0, 0.0, 0.0, 1.0);
        let position = position.truncate() / position.w;
        // Local east, north and up map onto the eye's right, up and back: the eye faces down.
        let view = Matrix4::from_scale(eye.scale) * Matrix4::from_translation(-position);
        let frustum = EyeFrustum {
            left: reach,
            right: reach,
            top: reach,
            bottom: reach,
            ..eye.frustum.scaled(eye.scale)
        };
        let mut below = self.clone();
        below
            .set_external_view(
                ExternalView {
                    anchor: eye.anchor,
                    view,
                    frustum,
                },
                projection,
            )
            .ok()?;
        Some(below)
    }

    /// The external eye as the globe camera takes it, if an external view is in effect.
    pub fn external_globe_eye(&self) -> Option<ExternalGlobeEye> {
        let eye = self.external_eye?;
        Some(ExternalGlobeEye {
            position: eye.sphere.position,
            axes: eye.sphere.axes,
            frustum: eye.frustum.scaled(self.pixels_per_meter()),
            camera_to_center_distance: self.camera_to_center_distance(),
        })
    }
}

/// Sine of the pitch below which a view counts as looking straight down.
const LEVEL_PITCH_THRESHOLD: f64 = 1e-6;
/// An eye on the ground still sees this far into the haze.
const MIN_FOG_HEIGHT_METERS: f64 = 100.0;
/// Bounds on the far fog distance, so the haze neither closes in nor vanishes at odd heights.
const MIN_FOG_FAR_METERS: f64 = 20_000.0;
const MAX_FOG_FAR_METERS: f64 = 600_000.0;
/// The near fog distance is the far one over this.
const FOG_FAR_TO_NEAR: f64 = 16.0;

/// Where an eye is and which way it faces, in the local frame of an [`ExternalView`].
#[derive(Clone, Copy, Debug)]
pub(super) struct EyeFrame {
    /// The eye in local-frame metres.
    pub position: Vector3<f64>,
    /// Unit vector along the eye's x axis.
    pub right: Vector3<f64>,
    /// Unit vector along the eye's y axis.
    pub up: Vector3<f64>,
    /// Unit vector along the eye's z axis, which points away from what it looks at.
    pub back: Vector3<f64>,
    /// Eye space units per local-frame metre.
    pub scale: f64,
}

impl EyeFrame {
    fn of(view: &Matrix4<f64>) -> Result<Self, ExternalViewError> {
        let cols: [[f64; 4]; 4] = (*view).into();
        if cols.iter().flatten().any(|value| !value.is_finite()) {
            return Err(ExternalViewError::SingularView);
        }
        let inverse = view.invert().ok_or(ExternalViewError::SingularView)?;
        let eye = inverse * Vector4::new(0.0, 0.0, 0.0, 1.0);
        let scale = (view * Vector4::new(1.0, 0.0, 0.0, 0.0))
            .truncate()
            .magnitude();
        if eye.w.abs() <= f64::EPSILON || !scale.is_finite() || scale <= 0.0 {
            return Err(ExternalViewError::SingularView);
        }
        let axis =
            |direction: Vector3<f64>| (inverse * direction.extend(0.0)).truncate().normalize();
        Ok(Self {
            position: eye.truncate() / eye.w,
            right: axis(Vector3::unit_x()),
            up: axis(Vector3::unit_y()),
            back: axis(Vector3::unit_z()),
            scale,
        })
    }

    /// Where the eye looks.
    pub fn forward(&self) -> Vector3<f64> {
        -self.back
    }
}

/// Bearing, pitch and roll of an eye whose right and forward directions are given in a
/// frame with x east, y north and z up.
pub(super) fn view_angles(
    right: Vector3<f64>,
    forward: Vector3<f64>,
) -> (Rad<f64>, Rad<f64>, Rad<f64>) {
    let pitch = (-forward.z).clamp(-1.0, 1.0).acos();
    // Looking straight down, the forward direction says nothing about the bearing; the right
    // vector, which lies on the ground, still does. The threshold sits above the rounding of
    // an arc cosine near one, which leaves a straight-down view a few hundred nanoradians of
    // pitch.
    let bearing = if pitch.sin() > LEVEL_PITCH_THRESHOLD {
        forward.x.atan2(forward.y)
    } else {
        (-right.y).atan2(right.x)
    };
    let level_right = Vector3::new(bearing.cos(), -bearing.sin(), 0.0);
    let level_up = level_right.cross(forward);
    // The map turns the view by minus the roll about its axis, which is where the sign comes
    // from.
    let roll = (-right.dot(level_up)).atan2(right.dot(level_right));
    (Rad(bearing), Rad(pitch), Rad(roll))
}

/// The camera pose an eye describes on the flat map, where the local frame's axes are the
/// map's.
fn flat_pose_of(frame: &EyeFrame, anchor: ExternalAnchor, body: Body) -> CameraPose {
    let (bearing, pitch, roll) = view_angles(frame.right, frame.forward());
    let units_per_meter =
        1.0 / (body.circumference_meters() * anchor.position.latitude.to_radians().cos());
    let anchor_mercator = mercator_from_lat_lon(anchor.position);
    CameraPose {
        position: lat_lon_at_mercator(Point2::new(
            anchor_mercator.x + frame.position.x * units_per_meter,
            anchor_mercator.y - frame.position.y * units_per_meter,
        )),
        altitude_meters: anchor.altitude_meters + frame.position.z,
        bearing: bearing.into(),
        pitch: pitch.into(),
        roll: roll.into(),
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "external/regression/tests.rs"]
mod regression;

#[cfg(test)]
#[path = "external/unprojection/tests.rs"]
mod unprojection_tests;

#[cfg(test)]
#[path = "external/below_msl/tests.rs"]
mod below_msl_tests;
