//! Enforces camera eye and perspective near-plane clearance above 3D terrain.
//!
//! Sampling a single point at `(eye.x, eye.y)` with zero vertical margin allows the perspective
//! near clip quad (`near_z = height / 50`) to slice triangular holes into steep `60°–75°` Alpine
//! ridges (such as Nordkette above Innsbruck or the Matterhorn) and traps constant-center orbit
//! cameras when `min_zoom` or `max_pitch` clamps a single-step lift. Here a 5-point footprint
//! spanning the eye and the 4 horizontal corners of the near clip quad is checked against both
//! the finest loaded DEM and the stitched rendered surface, with a positive clearance scaled to
//! the near-plane world radius.

use cgmath::{EuclideanSpace, InnerSpace, Rad, Vector2, Vector3};

use super::{camera_ground_position, circumference_at_latitude, globe};
use crate::{
    coords::{Zoom, TILE_SIZE},
    render::{projection::mercator_world_to_lat_lon, view_state::ViewState},
    style::Style,
    tcs::{tiles::Tiles, world::World},
    terrain::{coverage::TerrainCoverageIndex, DemDetail},
};

/// Minimum vertical eye clearance above terrain even at street-level zooms.
const MIN_TERRAIN_CLEARANCE_METERS: f64 = 12.0;
/// Multiplier on the 3D world radius of the perspective near clip quad.
const NEAR_PLANE_SAFETY_FACTOR: f64 = 0.85;
/// Extra margin above the vertical reach of the lowest near-plane corner.
const NEAR_CORNER_DROP_FACTOR: f64 = 1.15;
/// Small cushion added when lifting so an immediate re-check is strictly clear without chatter.
const LIFT_EPSILON_METERS: f64 = 0.05;
/// Maximum fixed-point reconciliation passes across coupled `(zoom, pitch)` changes.
const MAX_CLEARANCE_PASSES: usize = 6;

/// Minimum vertical clearance in metres required between [`camera_ground_position`] and the
/// highest terrain sample across the eye's near-clip footprint.
pub fn terrain_eye_clearance_meters(view_state: &ViewState) -> f64 {
    let (width, height) = view_state.viewport_size();
    let safe_h = height.max(1.0);
    let near_z_px = safe_h / 50.0;
    let ppm = view_state.pixels_per_meter().max(1e-9);
    let near_z_m = near_z_px / ppm;

    let tan_half_y = (view_state.field_of_view().0 * 0.5).tan().abs();
    let tan_half_x = tan_half_y * (width.max(1.0) / safe_h);
    let quad_radius_m = near_z_m * tan_half_x.hypot(tan_half_y);
    let near_plane_world_radius = near_z_m.hypot(quad_radius_m);

    let pitch = view_state.camera().get_pitch().0.abs();
    let near_corner_drop = near_z_m * pitch.cos() + quad_radius_m * pitch.sin();

    MIN_TERRAIN_CLEARANCE_METERS
        .max(near_plane_world_radius * NEAR_PLANE_SAFETY_FACTOR)
        .max(near_corner_drop * NEAR_CORNER_DROP_FACTOR)
}

/// Samples the highest terrain elevation across the 5-point camera + near-plane quad footprint.
pub(crate) fn footprint_max_ground(
    view_state: &ViewState,
    index: &TerrainCoverageIndex,
    tiles: &Tiles,
) -> Option<f64> {
    if index.is_empty() {
        return None;
    }
    let world_size = TILE_SIZE * 2_f64.powf(view_state.zoom().value());
    if !world_size.is_finite() || world_size <= 0.0 {
        return None;
    }
    let (camera, _) = camera_ground_position(view_state);
    let (width, height) = view_state.viewport_size();
    let safe_h = height.max(1.0);
    let near_z_px = safe_h / 50.0;
    let tan_half_y = (view_state.field_of_view().0 * 0.5).tan().abs();
    let half_h_px = near_z_px * tan_half_y;
    let half_w_px = half_h_px * (width.max(1.0) / safe_h);

    let pitch = view_state.camera().get_pitch().0;
    let bearing = view_state.camera().get_bearing().0;
    // Horizontal forward unit vector from camera toward map center, and right unit vector.
    let fwd = Vector2::new(bearing.sin(), -bearing.cos());
    let right = Vector2::new(bearing.cos(), bearing.sin());

    let near_fwd_bottom = near_z_px * pitch.sin() + half_h_px * pitch.cos();
    let near_fwd_top = near_z_px * pitch.sin() - half_h_px * pitch.cos();

    let points = [
        camera,
        camera + fwd * near_fwd_bottom - right * half_w_px,
        camera + fwd * near_fwd_bottom + right * half_w_px,
        camera + fwd * near_fwd_top - right * half_w_px,
        camera + fwd * near_fwd_top + right * half_w_px,
    ];

    let mut max_ground: Option<f64> = None;
    for pt in points {
        let mx = pt.x / world_size;
        let my = pt.y / world_size;
        if let Some(h) = sample_best_ground(index, tiles, mx, my) {
            max_ground = Some(max_ground.map_or(h, |prev| prev.max(h)));
        }
    }
    max_ground
}

fn sample_best_ground(
    index: &TerrainCoverageIndex,
    tiles: &Tiles,
    mercator_x: f64,
    mercator_y: f64,
) -> Option<f64> {
    if !(0.0..1.0).contains(&mercator_y) || !mercator_x.is_finite() {
        return None;
    }
    let finest = index.loaded_ground_cached(tiles, mercator_x, mercator_y, DemDetail::Finest);
    let stitched = index.elevation_at(tiles, mercator_x, mercator_y);
    match (finest, stitched) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn footprint_deficit(
    view_state: &ViewState,
    index: &TerrainCoverageIndex,
    tiles: &Tiles,
    extra_margin: f64,
) -> Option<(f64, f64, f64)> {
    let (_, altitude) = camera_ground_position(view_state);
    let max_ground = footprint_max_ground(view_state, index, tiles)?;
    let required = max_ground + terrain_eye_clearance_meters(view_state) + extra_margin;
    Some((altitude, max_ground, required))
}

/// Lowers camera pitch along the constant-zoom orbit sphere when the current pitch places the
/// camera or its near-clip quad below the loaded terrain, keeping `zoom` and `center` unchanged.
/// Returns whether the pitch was adjusted.
pub fn clamp_pitch_above_terrain_keeping_zoom(
    style: &Style,
    view_state: &mut ViewState,
    world: &World,
) -> bool {
    if style.terrain.is_none() || globe::globe_pose(view_state).is_some() {
        return false;
    }
    let Some(index) = world.resources.get::<TerrainCoverageIndex>() else {
        return false;
    };
    let current_pitch = view_state.camera().get_pitch().0;
    if current_pitch <= 1e-4 {
        return false;
    }
    let is_clear = |view: &ViewState, extra: f64| -> Option<bool> {
        let (altitude, _, required) = footprint_deficit(view, index, &world.tiles, extra)?;
        Some(altitude >= required)
    };
    let Some(false) = is_clear(view_state, 0.0) else {
        return false;
    };
    view_state.camera_mut().set_pitch(Rad(0.0));
    if is_clear(view_state, LIFT_EPSILON_METERS) != Some(true) {
        view_state.camera_mut().set_pitch(Rad(current_pitch));
        return false;
    }
    let mut lo = 0.0_f64;
    let mut hi = current_pitch;
    for _ in 0..12 {
        let mid = 0.5 * (lo + hi);
        view_state.camera_mut().set_pitch(Rad(mid));
        if is_clear(view_state, LIFT_EPSILON_METERS).unwrap_or(true) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    view_state.camera_mut().set_pitch(Rad(lo));
    true
}

/// Lifts the camera above the terrain under its 5-point near-clip footprint by reconciling pitch
/// and zoom while keeping the orbit center in place. Returns whether the camera moved.
pub fn keep_camera_above_terrain(style: &Style, view_state: &mut ViewState, world: &World) -> bool {
    if style.terrain.is_none() {
        return false;
    }
    if globe::globe_pose(view_state).is_some() {
        return globe::keep_globe_camera_above_terrain(view_state, world);
    }
    let Some(index) = world.resources.get::<TerrainCoverageIndex>() else {
        return false;
    };
    let mut moved = false;
    for _ in 0..MAX_CLEARANCE_PASSES {
        let Some((altitude, max_ground, required)) =
            footprint_deficit(view_state, index, &world.tiles, 0.0)
        else {
            break;
        };
        if altitude >= required {
            break;
        }
        let prev_zoom = view_state.zoom().value();
        let prev_pitch = view_state.camera().get_pitch().0;
        let target_altitude = required + LIFT_EPSILON_METERS;

        tracing::debug!(
            altitude,
            max_ground,
            required,
            zoom = prev_zoom,
            pitch = prev_pitch.to_degrees(),
            center_elevation = view_state.center_elevation(),
            "camera inside terrain clearance envelope; lifting it"
        );

        lift_eye_at_constant_xy(view_state, target_altitude);

        // If `min_zoom` or `max_pitch` prevented the constant-XY lift from clearing the ridge,
        // tilt toward nadir and/or back zoom out along the view ray so the eye never stays trapped.
        if footprint_deficit(view_state, index, &world.tiles, 0.0)
            .is_some_and(|(alt, _, req)| alt < req)
        {
            if !clamp_pitch_above_terrain_keeping_zoom(style, view_state, world) {
                back_off_zoom_and_pitch(view_state, index, &world.tiles);
            }
        }

        let changed = (view_state.zoom().value() - prev_zoom).abs() > 1e-9
            || (view_state.camera().get_pitch().0 - prev_pitch).abs() > 1e-9;
        if !changed {
            break;
        }
        moved = true;
    }
    moved
}

fn lift_eye_at_constant_xy(view_state: &mut ViewState, target_altitude: f64) {
    let world_size = TILE_SIZE * 2_f64.powf(view_state.zoom().value());
    let (camera, _) = camera_ground_position(view_state);
    let center = view_state.camera().position().to_vec();
    let center_elevation = view_state.center_elevation();
    let circumference = circumference_at_latitude(
        view_state.body(),
        mercator_world_to_lat_lon(center.x, center.y, world_size).latitude,
    );
    let from = Vector3::new(
        camera.x / world_size,
        camera.y / world_size,
        target_altitude / circumference,
    );
    let to = Vector3::new(
        center.x / world_size,
        center.y / world_size,
        center_elevation / circumference,
    );
    let delta = to - from;
    let distance_3d = delta.magnitude();
    if distance_3d <= f64::EPSILON {
        return;
    }
    let ground = delta.x.hypot(delta.y);
    let zoom = (view_state.camera_to_center_distance() / distance_3d / TILE_SIZE).log2();
    let mut pitch = (ground / distance_3d).clamp(-1.0, 1.0).acos();
    pitch = if delta.z < 0.0 {
        std::f64::consts::FRAC_PI_2 - pitch
    } else {
        std::f64::consts::FRAC_PI_2 + pitch
    };
    view_state.camera_mut().set_pitch(Rad(pitch));
    view_state.zoom_to(Zoom::new(zoom));
}

fn back_off_zoom_and_pitch(
    view_state: &mut ViewState,
    index: &TerrainCoverageIndex,
    tiles: &Tiles,
) {
    // Reduce pitch toward nadir first if pitch is steep, then step zoom out so orbit radius grows.
    let reduced_pitch = (view_state.camera().get_pitch().0 * 0.5).min(0.6);
    view_state.camera_mut().set_pitch(Rad(reduced_pitch));
    let Some((_, _, required)) = footprint_deficit(view_state, index, tiles, LIFT_EPSILON_METERS)
    else {
        return;
    };
    let cos_p = view_state.camera().get_pitch().0.cos().max(0.25);
    let needed_vertical_m = (required - view_state.center_elevation()).max(10.0);
    let needed_dist_m = needed_vertical_m / cos_p;
    let current_dist_m =
        view_state.camera_to_center_distance() / view_state.pixels_per_meter().max(1e-9);
    if needed_dist_m > current_dist_m && current_dist_m > 0.0 {
        let zoom_shift = (needed_dist_m / current_dist_m).log2();
        let target_zoom = view_state.zoom().value() - zoom_shift;
        view_state.zoom_to(Zoom::new(target_zoom));
    }
    if footprint_deficit(view_state, index, tiles, 0.0).is_some_and(|(alt, _, req)| alt < req) {
        view_state.camera_mut().set_pitch(Rad(0.0));
    }
}
