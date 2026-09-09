//! Bounded camera prediction for terrain requests; displayed poses remain unpredicted.
use std::time::Duration;

use cgmath::{InnerSpace, Matrix3, Matrix4, Quaternion, SquareMatrix, Vector3};

use crate::{
    projection::ProjectionType,
    render::view_state::{ExternalView, ViewState},
};

const SAMPLE_INTERVAL: f64 = 0.1;
const LOOK_AHEAD: f64 = 0.35;
const MAX_TURN: f64 = 20.0 * std::f64::consts::PI / 180.0;

#[derive(Default)]
pub(super) struct MotionPrefetch {
    sample: Option<(Duration, ExternalView)>,
    ahead: Option<ViewState>,
}

impl MotionPrefetch {
    pub(super) fn update(
        &mut self,
        timestamp: Duration,
        current: ExternalView,
        base: &ViewState,
        projection: &ProjectionType,
    ) -> Option<ViewState> {
        let Some((time, previous)) = self.sample else {
            self.sample = Some((timestamp, current));
            return None;
        };
        let dt = timestamp.saturating_sub(time).as_secs_f64();
        if timestamp > time && dt < SAMPLE_INTERVAL {
            return self.ahead.clone();
        }
        self.sample = Some((timestamp, current));
        self.ahead = (dt >= SAMPLE_INTERVAL && dt <= 0.5)
            .then(|| predict(previous, current, dt, base, projection))
            .flatten();
        self.ahead.clone()
    }
}

fn predict(
    previous: ExternalView,
    current: ExternalView,
    dt: f64,
    base: &ViewState,
    projection: &ProjectionType,
) -> Option<ViewState> {
    let now = current.view.invert()?;
    let before = previous.view.invert()?;
    let scale = now.x.truncate().magnitude();
    if (before.x.truncate().magnitude() / scale - 1.0).abs() > 0.05 {
        return None;
    }
    let orientation = |matrix: Matrix4<f64>| {
        Quaternion::from(Matrix3::from_cols(
            matrix.x.truncate().normalize(),
            matrix.y.truncate().normalize(),
            matrix.z.truncate().normalize(),
        ))
        .normalize()
    };
    let (rotation, old_rotation) = (orientation(now), orientation(before));
    let angle = 2.0 * rotation.dot(old_rotation).abs().clamp(0.0, 1.0).acos();
    let turn_factor = (LOOK_AHEAD / dt).min(MAX_TURN / angle.max(1e-9));
    let ahead_rotation = old_rotation.slerp(rotation, 1.0 + turn_factor).normalize();
    let offset = anchor_offset(previous, current, base.body().radius_meters)?;
    let position = now.w.truncate();
    let travel = (position - before.w.truncate() - offset) * (LOOK_AHEAD / dt);
    let distance = travel.magnitude();
    let reach = position.z.abs().max(100.0) * 2.0;
    let travel = travel * (reach / distance.max(reach));
    if angle < 0.002 && distance < 1.0 {
        return None;
    }
    let local_from_eye = Matrix4::from_translation(position + travel)
        * Matrix4::from(ahead_rotation)
        * Matrix4::from_scale(scale);
    let mut ahead = base.clone();
    ahead
        .set_external_view(
            ExternalView {
                view: local_from_eye.invert()?,
                ..current
            },
            projection,
        )
        .ok()?;
    if projection.uses_globe_rendering(ahead.zoom().value()) {
        return None;
    }
    Some(ahead)
}

fn anchor_offset(
    previous: ExternalView,
    current: ExternalView,
    radius: f64,
) -> Option<Vector3<f64>> {
    let before = previous.anchor.position;
    let now = current.anchor.position;
    let longitude = (before.longitude - now.longitude + 180.0).rem_euclid(360.0) - 180.0;
    if longitude.abs() > 0.5 || (before.latitude - now.latitude).abs() > 0.5 {
        return None;
    }
    let latitude = now.latitude.to_radians();
    let north = |degrees: f64| degrees.to_radians().tan().asinh();
    Some(Vector3::new(
        longitude.to_radians() * radius * latitude.cos(),
        (north(before.latitude) - north(now.latitude)) * radius * latitude.cos(),
        previous.anchor.altitude_meters - current.anchor.altitude_meters,
    ))
}

#[cfg(test)]
mod tests;
