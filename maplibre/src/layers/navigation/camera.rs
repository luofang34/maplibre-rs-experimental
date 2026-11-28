//! Camera moves shared by navigation actions and host gesture handlers.

use cgmath::{EuclideanSpace, Point2, Rad, Vector2};

use super::projection::active_globe_camera;
use crate::{
    render::view_state::ViewState,
    terrain::interaction::{center_pixel, set_location_at_pixel},
};

fn is_free_globe(view_state: &ViewState) -> bool {
    view_state.navigation_mode() == crate::render::view_state::NavigationMode::FreeGlobe
}

fn try_orbit_free_globe(
    view_state: &mut ViewState,
    yaw: Rad<f64>,
    pitch: Rad<f64>,
    warn_msg: Option<&str>,
) -> bool {
    let Some(pose) = view_state.globe_pose() else {
        return false;
    };
    if let Err(error) = view_state.orbit_globe_pose(pose, yaw, pitch) {
        if let Some(msg) = warn_msg {
            tracing::warn!(%error, "{msg}");
        }
    }
    true
}

fn rotate_globe_around_pivot(
    style: &crate::style::Style,
    view_state: &mut ViewState,
    pivot: Point2<f64>,
    delta: Rad<f64>,
) -> bool {
    let Some(before_globe) = active_globe_camera(style, view_state) else {
        return false;
    };
    let anchor = before_globe.screen_point_to_location(pivot);
    let current = view_state.camera().get_bearing();
    view_state.camera_mut().set_bearing(current + delta);
    if let (Some(anchor), Some(after_globe)) = (anchor, active_globe_camera(style, view_state))
        && let Some(under) = after_globe.screen_point_to_location(pivot)
    {
        let update = crate::projection::globe::interaction::pan_center_to_anchor(
            after_globe.center(),
            after_globe.bearing_degrees(),
            anchor,
            under,
        );
        let zoom = view_state.zoom();
        let world_center = crate::coords::WorldCoords::from_lat_lon(update.center, zoom);
        view_state
            .camera_mut()
            .move_to(Point2::new(world_center.x, world_center.y));
    }
    true
}

fn rotate_flat_around_pivot(view_state: &mut ViewState, pivot: Point2<f64>, delta: Rad<f64>) {
    let plane = view_state.center_elevation();
    let before = view_state
        .inverted_view_projection()
        .ok()
        .and_then(|inv| view_state.window_to_world_at_elevation(&pivot.to_vec(), &inv, plane));
    let current = view_state.camera().get_bearing();
    view_state.camera_mut().set_bearing(current + delta);
    if let Some(before) = before {
        set_location_at_pixel(view_state, before, plane, pivot);
    }
}

/// Sets the camera pitch while keeping the ground point at the viewport center pinned.
pub fn set_pitch_keeping_center(view_state: &mut ViewState, pitch: impl Into<Rad<f64>>) {
    let pitch: Rad<f64> = pitch.into();
    if is_free_globe(view_state) {
        let current = view_state.camera().get_pitch();
        if try_orbit_free_globe(view_state, Rad(0.0), pitch - current, None) {
            return;
        }
    }
    view_state.camera_mut().set_pitch(pitch);
}

/// Sets the camera bearing while keeping the ground point at the viewport center pinned.
pub fn set_bearing_keeping_center(view_state: &mut ViewState, bearing: impl Into<Rad<f64>>) {
    let bearing: Rad<f64> = bearing.into();
    if is_free_globe(view_state) {
        let current = view_state.camera().get_bearing();
        if try_orbit_free_globe(
            view_state,
            bearing - current,
            Rad(0.0),
            Some("free globe bearing rejected"),
        ) {
            return;
        }
    }
    view_state.camera_mut().set_bearing(bearing);
}

/// Rotates camera bearing by `delta` around the ground point under `cursor` (falling back to
/// the viewport center when the cursor is off the surface).
pub fn rotate_bearing_around_cursor(
    style: &crate::style::Style,
    view_state: &mut ViewState,
    cursor: Option<Vector2<f64>>,
    delta: Rad<f64>,
) {
    if is_free_globe(view_state) {
        try_orbit_free_globe(view_state, delta, Rad(0.0), None);
        return;
    }
    let center = center_pixel(view_state);
    let pivot = cursor.map_or(center, |c| Point2::new(c.x, c.y));

    if rotate_globe_around_pivot(style, view_state, pivot, delta) {
        return;
    }

    rotate_flat_around_pivot(view_state, pivot, delta);
}

fn smoothstep_stop(z0: f64, p0: f64, z1: f64, p1: f64, zoom: f64) -> f64 {
    let t = ((zoom - z0) / (z1 - z0)).clamp(0.0, 1.0);
    let s = t * t * (3.0 - 2.0 * t);
    p0 + (p1 - p0) * s
}

/// Base Auto-Tilt curve in degrees as a function of continuous zoom.
///
/// Smoothly transitions from `0°` (overhead) at `zoom <= 7.5` through regional (`25°` at `z9.0`),
/// mountain range (`50°` at `z10.5`), alpine overview (`68°` at `z12.0`, matching the 3D terrain
/// demo), valley (`73°` at `z14.0`), up to `76°` at `zoom >= 16.0`.
pub(super) fn base_auto_tilt_degrees(zoom: f64) -> f64 {
    const STOPS: &[(f64, f64)] = &[
        (7.5, 0.0),
        (9.0, 25.0),
        (10.5, 50.0),
        (12.0, 68.0),
        (14.0, 73.0),
        (16.0, 76.0),
    ];
    if let Some(&(z0, p0)) = STOPS.first() {
        if zoom <= z0 {
            return p0;
        }
    }
    for window in STOPS.windows(2) {
        let &[(z0, p0), (z1, p1)] = window else {
            continue;
        };
        if zoom <= z1 {
            return smoothstep_stop(z0, p0, z1, p1, zoom);
        }
    }
    STOPS.last().map_or(0.0, |&(_, p)| p)
}

/// Normalizes an angle in radians to `[-PI, PI]`.
pub fn normalize_angle_signed(angle: f64) -> f64 {
    let two_pi = std::f64::consts::TAU;
    let mut a = (angle + std::f64::consts::PI).rem_euclid(two_pi) - std::f64::consts::PI;
    if a <= -std::f64::consts::PI {
        a += two_pi;
    }
    a
}
