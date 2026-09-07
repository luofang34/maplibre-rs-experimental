//! An external eye on the unit sphere: where the globe camera stands when the host places
//! the eye.

use cgmath::{Deg, InnerSpace, Matrix3, Vector3};

use super::{view_angles, ExternalAnchor, EyeFrame};
use crate::{
    coords::LatLon,
    projection::{
        body::Body,
        globe::{angular_radians_to_unit_sphere, unit_sphere_to_lat_lon},
    },
};

/// An eye in unit-sphere coordinates.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SphereEye {
    /// Where the eye is; a length of one sits on the surface.
    pub position: Vector3<f64>,
    /// Axes of the eye's space, columns x right, y up and z backwards.
    pub axes: Matrix3<f64>,
}

/// The map center and angles an eye on the sphere stands for.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SpherePose {
    /// Where the eye's view axis meets the sphere, or the point beneath the eye when it
    /// misses.
    pub center: LatLon,
    /// Distance from the eye to the center in metres.
    pub distance_meters: f64,
    /// Bearing, clockwise from north at the center.
    pub bearing: Deg<f64>,
    /// Pitch away from looking straight down at the center.
    pub pitch: Deg<f64>,
    /// Roll about the view axis.
    pub roll: Deg<f64>,
}

impl SphereEye {
    /// Places the eye of a local frame anchored at `anchor` on the unit sphere.
    pub(crate) fn place(frame: &EyeFrame, anchor: ExternalAnchor, body: Body) -> Self {
        let (east, north, up) = local_axes(anchor.position);
        let to_sphere = Matrix3::from_cols(east, north, up);
        let origin = up * body.unit_radius_at(anchor.altitude_meters);
        Self {
            position: origin + to_sphere * frame.position / body.radius_meters,
            axes: Matrix3::from_cols(
                to_sphere * frame.right,
                to_sphere * frame.up,
                to_sphere * frame.back,
            ),
        }
    }

    /// The center and angles the eye stands for, the angles taken in the center's own east,
    /// north and up frame as the globe camera measures them.
    pub(crate) fn pose(&self, body: Body) -> SpherePose {
        let forward = -self.axes.z;
        let center = gaze_center(self.position, forward);
        let location = unit_sphere_to_lat_lon(center);
        let (east, north, up) = local_axes(location);
        let in_local = |v: Vector3<f64>| Vector3::new(v.dot(east), v.dot(north), v.dot(up));
        let (bearing, pitch, roll) = view_angles(in_local(self.axes.x), in_local(forward));
        SpherePose {
            center: location,
            distance_meters: (self.position - center).magnitude() * body.radius_meters,
            bearing: bearing.into(),
            pitch: pitch.into(),
            roll: roll.into(),
        }
    }
}

/// Unit vectors east, north and up at a location, in unit-sphere coordinates.
fn local_axes(location: LatLon) -> (Vector3<f64>, Vector3<f64>, Vector3<f64>) {
    let longitude = location.longitude.to_radians();
    let latitude = location.latitude.to_radians();
    let up = angular_radians_to_unit_sphere(longitude, latitude);
    let east = Vector3::new(longitude.cos(), 0.0, -longitude.sin());
    let north = Vector3::new(
        -longitude.sin() * latitude.sin(),
        latitude.cos(),
        -longitude.cos() * latitude.sin(),
    );
    (east, north, up)
}

/// How far along the gaze the center may lie, as a multiple of the eye's height above the
/// surface. A grazing gaze meets the surface near the horizon, and a center there would
/// have the covering keep tiles near the horizon and drop the ground beneath the eye.
const GAZE_REACH_HEIGHTS: f64 = 2.0;
/// Radii past the limb over which a missing gaze's center moves from the limb point to the
/// point beneath the eye.
const MISS_BLEND_RADII: f64 = 0.25;

/// The center of an eye at `origin` looking along `direction`: where the gaze meets the unit
/// sphere, brought back to within reach along the gaze when it meets it further away, and
/// for a gaze that misses the sphere the limb point nearest to it, settling on the point
/// beneath the eye a quarter radius further out.
fn gaze_center(origin: Vector3<f64>, direction: Vector3<f64>) -> Vector3<f64> {
    let reach = GAZE_REACH_HEIGHTS * (origin.magnitude() - 1.0).max(0.0);
    match first_surface_hit(origin, direction) {
        Some(hit) if (hit - origin).magnitude() <= reach => hit,
        Some(_) => {
            let ahead = origin + direction * reach;
            let length = ahead.magnitude();
            if length > 0.0 {
                ahead / length
            } else {
                origin / origin.magnitude()
            }
        }
        None => missed_center(origin, direction),
    }
}

/// The center for a ray from `origin` along `direction` that misses the unit sphere: the
/// limb point under the ray's closest approach when the ray just misses, the point beneath
/// the eye once it misses by more than a quarter radius, and a blend between.
fn missed_center(origin: Vector3<f64>, direction: Vector3<f64>) -> Vector3<f64> {
    let beneath = origin / origin.magnitude();
    let closest = origin - direction * origin.dot(direction);
    let length = closest.magnitude();
    if length <= 0.0 {
        return beneath;
    }
    let limb = closest / length;
    let share = ((length - 1.0) / MISS_BLEND_RADII).clamp(0.0, 1.0);
    let blended = limb * (1.0 - share) + beneath * share;
    let magnitude = blended.magnitude();
    if magnitude > 0.0 {
        blended / magnitude
    } else {
        beneath
    }
}

/// Where a ray from `origin` along `direction` first meets the unit sphere.
fn first_surface_hit(origin: Vector3<f64>, direction: Vector3<f64>) -> Option<Vector3<f64>> {
    let along = origin.dot(direction);
    let discriminant = along * along - (origin.magnitude2() - 1.0);
    if discriminant < 0.0 {
        return None;
    }
    let distance = -along - discriminant.sqrt();
    (distance >= 0.0).then(|| origin + direction * distance)
}
