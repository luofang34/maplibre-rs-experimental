//! Terrain-aware camera interaction, following GL JS's handler manager and camera helpers.
//!
//! Gestures anchor on the terrain surface under the pointer rather than on the sea-level
//! plane, fall back to the view center while the pointer is in the sky, hold the center
//! elevation still while they run and reconcile it when they end, and the camera is lifted
//! out of the ground after every change.

use cgmath::{EuclideanSpace, InnerSpace, Point2, Vector2};

use crate::{
    coords::{LatLon, Zoom, TILE_SIZE},
    projection::{body::Body, globe::camera::GlobeCameraState},
    render::{
        projection::{globe_camera_for_view, mercator_world_to_lat_lon},
        view_state::ViewState,
    },
    style::Style,
    tcs::{tiles::Tiles, world::World},
    terrain::{
        coverage::{TerrainCoverageIndex, TerrainSample},
        sightline::{pick_globe_terrain, DrawnTerrain, TerrainPick},
    },
};

/// Passes that reconcile a finished gesture's center with the ground it reaches; each leaves
/// the next a change of the ground over a shorter stretch of the view.
const GESTURE_GROUND_PASSES: usize = 8;
/// How near the ground under a finished gesture's center its elevation must stand.
const GESTURE_GROUND_TOLERANCE_METERS: f64 = 1e-3;
/// World pixels between ray samples on the flat map.
const TARGET_WORLD_STEP_PX: f64 = 4.0;
const MAX_SAMPLES: usize = 512;
const MERCATOR_BISECT_EPSILON_WORLD_PX: f64 = 1e-3;
const MAX_BISECTIONS: usize = 40;
const HIT_EPSILON_METERS: f64 = 1e-6;
/// An anchor this close to the camera's altitude above the center is unusable for a gesture.
const TERRAIN_ANCHOR_MAX_CAMERA_ALTITUDE_FRACTION: f64 = 0.9;
pub(crate) use crate::render::view_state::{
    camera_direction, distance_to_center_from_altitude, DISTANCE_TO_CENTER_WHEN_LOOKING_UP_METERS,
};
pub use crate::{render::view_state::camera_ground_position, terrain::sightline::TerrainHit};

/// Anchor of a zoom or drag gesture, as GL JS `_resolveAround` and `_terrainGestureElevation`
/// pick it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GestureAnchor {
    /// Screen pixel the gesture keeps its ground point under.
    pub pixel: Point2<f64>,
    /// Elevation of the anchored terrain point when usable; `None` anchors on the plane at the
    /// center elevation.
    pub elevation: Option<f64>,
}

/// Screen pixel of the view center, honouring edge insets.
pub fn center_pixel(view_state: &ViewState) -> Point2<f64> {
    view_state
        .edge_insets()
        .center(view_state.width(), view_state.height())
}

/// Whether the style renders the globe at the current zoom.
fn uses_globe(style: &Style, view_state: &ViewState) -> bool {
    style.projection.as_ref().is_some_and(|projection| {
        projection
            .projection_type
            .uses_globe_rendering(view_state.zoom().value())
    })
}

fn is_below_terrain(sample: TerrainSample, height: f64) -> bool {
    sample.covered && height <= sample.elevation + HIT_EPSILON_METERS
}

fn bisect(
    mut below: impl FnMut(f64) -> bool,
    mut lo: f64,
    mut hi: f64,
    tolerance: f64,
) -> (f64, f64) {
    for _ in 0..MAX_BISECTIONS {
        if hi - lo <= tolerance {
            break;
        }
        let mid = (lo + hi) / 2.0;
        if below(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    (lo, hi)
}

/// Casts the ray through a pixel of the flat map against the rendered terrain.
pub fn screen_point_to_terrain_mercator(
    view_state: &ViewState,
    index: &TerrainCoverageIndex,
    tiles: &Tiles,
    pixel: Point2<f64>,
) -> Option<TerrainHit> {
    if index.is_empty() {
        return None;
    }
    let inverted = view_state.inverted_view_projection().ok()?;
    let window = pixel.to_vec();
    let near = view_state.window_to_world_at_depth(&window, 0.0, &inverted);
    let far = view_state.window_to_world_at_depth(&window, 1.0, &inverted);
    let world_size = TILE_SIZE * 2_f64.powf(view_state.zoom().value());
    let delta = far - near;
    let (mut t_start, mut t_end) = (0.0_f64, 1.0_f64);
    if delta.z == 0.0 {
        if near.z > index.max_elevation() || near.z < index.min_elevation() {
            return None;
        }
    } else {
        let t_high = (index.max_elevation() - near.z) / delta.z;
        let t_low = (index.min_elevation() - near.z) / delta.z;
        t_start = t_start.max(t_high.min(t_low));
        t_end = t_end.min(t_high.max(t_low));
        if t_start > t_end {
            return None;
        }
    }
    let horizontal = delta.x.hypot(delta.y);
    let samples = ((horizontal * (t_end - t_start) / TARGET_WORLD_STEP_PX).ceil() as usize)
        .clamp(1, MAX_SAMPLES);
    let sample_at = |t: f64| {
        index.sample(
            tiles,
            (near.x + t * delta.x) / world_size,
            (near.y + t * delta.y) / world_size,
        )
    };
    let below = |t: f64| is_below_terrain(sample_at(t), near.z + t * delta.z);
    let mut previous = 0.0;
    let mut above = !below(0.0);
    for step in 0..=samples {
        let t = t_start + (t_end - t_start) * step as f64 / samples as f64;
        if !above {
            above = !below(t);
        } else if below(t) {
            let (lo, hi) = bisect(
                below,
                previous,
                t,
                MERCATOR_BISECT_EPSILON_WORLD_PX / horizontal.max(f64::EPSILON),
            );
            let sample_lo = sample_at(lo);
            let sample_hi = sample_at(hi);
            let f_lo = near.z + lo * delta.z - sample_lo.elevation;
            let f_hi = near.z + hi * delta.z - sample_hi.elevation;
            let hit = if sample_lo.covered && f_lo > f_hi {
                (lo + f_lo * (hi - lo) / (f_lo - f_hi)).clamp(lo, hi)
            } else {
                hi
            };
            let mercator = Point2::new(
                (near.x + hit * delta.x) / world_size,
                (near.y + hit * delta.y) / world_size,
            );
            return Some(TerrainHit {
                mercator,
                location: LatLon::from_mercator(mercator),
                elevation: sample_at(hit).elevation,
            });
        }
        previous = t;
    }
    None
}

/// Casts the ray through a pixel of the globe against the rendered terrain.
///
/// `None` unless the ray meets drawn ground, a tile's or a polar cap's, before any ground no
/// DEM describes; [`pick_globe_terrain`] tells the sky and unknown ground apart.
pub fn screen_point_to_terrain_globe(
    camera: &GlobeCameraState,
    index: &TerrainCoverageIndex,
    tiles: &Tiles,
    pixel: Point2<f64>,
) -> Option<TerrainHit> {
    if index.is_empty() {
        return None;
    }
    let terrain = DrawnTerrain {
        index,
        tiles,
        body: camera.body(),
    };
    match pick_globe_terrain(camera, terrain, pixel) {
        TerrainPick::Ground(hit) | TerrainPick::PolarCap(hit) => Some(hit),
        TerrainPick::Sky | TerrainPick::Unknown => None,
    }
}

/// Casts the ray through a pixel against the rendered terrain of the active projection.
pub fn screen_point_to_terrain(
    style: &Style,
    view_state: &ViewState,
    world: &World,
    pixel: Point2<f64>,
) -> Option<TerrainHit> {
    style.terrain.as_ref()?;
    let index = world.resources.get::<TerrainCoverageIndex>()?;
    if uses_globe(style, view_state) {
        let camera = globe_camera_for_view(view_state).ok()?;
        screen_point_to_terrain_globe(&camera, index, &world.tiles, pixel)
    } else {
        screen_point_to_terrain_mercator(view_state, index, &world.tiles, pixel)
    }
}

/// Camera position in world pixels and its altitude in metres above sea level.
///
/// Picks the anchor of a gesture that started at `pointer`.
///
/// With terrain the pointer must rest on the surface, or the gesture anchors on the center; a
/// terrain point nearly as high as the camera is anchored on the center plane instead, so the
/// unprojection stays well conditioned.
pub fn resolve_gesture_anchor(
    style: &Style,
    view_state: &ViewState,
    world: &World,
    pointer: Point2<f64>,
) -> GestureAnchor {
    let center = center_pixel(view_state);
    if style.terrain.is_none() {
        return GestureAnchor {
            pixel: pointer,
            elevation: None,
        };
    }
    let Some(hit) = screen_point_to_terrain(style, view_state, world, pointer) else {
        return GestureAnchor {
            pixel: center,
            elevation: None,
        };
    };
    if (pointer - center).magnitude2() < 1e-2 {
        return GestureAnchor {
            pixel: pointer,
            elevation: None,
        };
    }
    let center_elevation = view_state.center_elevation();
    let (_, camera_altitude) = camera_ground_position(view_state);
    let usable = hit.elevation - center_elevation
        < TERRAIN_ANCHOR_MAX_CAMERA_ALTITUDE_FRACTION * (camera_altitude - center_elevation);
    GestureAnchor {
        pixel: pointer,
        elevation: usable.then_some(hit.elevation),
    }
}

mod clearance;
mod flat_gestures;
pub mod globe;
pub use clearance::{
    clamp_pitch_above_terrain_keeping_zoom, keep_camera_above_terrain, terrain_eye_clearance_meters,
};
pub use flat_gestures::{pan_mercator_by_pixels, set_location_at_pixel, zoom_mercator_around};

/// Holds the center elevation still for the duration of a gesture.
pub fn begin_gesture(view_state: &mut ViewState) {
    view_state.freeze_center_elevation();
}

/// Ends a gesture: the center elevation follows the terrain again and the zoom and center are
/// recomputed so the camera stays where the gesture left it.
pub fn finish_gesture(style: &Style, view_state: &mut ViewState, world: &World) {
    if !view_state.center_elevation_frozen() {
        return;
    }
    view_state.thaw_center_elevation();
    // Off the ground the center keeps its altitude; GL JS recomputes only a clamped center.
    if style.terrain.is_none() || !view_state.center_clamped_to_ground() {
        return;
    }
    // Keeping the eye moves the center along the view onto other ground, so the center is
    // reconciled again with the ground it reaches until the frame, which stands the center on
    // the ground under it, has nothing left to move.
    for _ in 0..GESTURE_GROUND_PASSES {
        let Some(elevation) = crate::terrain::elevation::center_ground(view_state, world) else {
            return;
        };
        if (elevation - view_state.center_elevation()).abs() < GESTURE_GROUND_TOLERANCE_METERS {
            return;
        }
        if globe::globe_pose(view_state).is_some() {
            if !globe::recalculate_globe_zoom_and_center(view_state, elevation) {
                return;
            }
        } else {
            recalculate_zoom_and_center(view_state, elevation);
        }
    }
}

/// Keeps the camera where it is while the center elevation changes to `elevation`: the center
/// slides along the view ray onto the new surface and the zoom follows the new distance, as GL
/// JS `recalculateZoomAndCenter` does.
pub fn recalculate_zoom_and_center(view_state: &mut ViewState, elevation: f64) {
    let current = view_state.center_elevation();
    if (current - elevation).abs() <= f64::EPSILON {
        return;
    }
    let world_size = TILE_SIZE * 2_f64.powf(view_state.zoom().value());
    let pixels_per_meter = view_state.pixels_per_meter();
    let pitch = view_state.camera().get_pitch().0;
    let bearing = view_state.camera().get_bearing().0;
    let (x, y, z) = camera_direction(pitch, bearing);
    let distance = view_state.camera_to_center_distance();
    let center = view_state.camera().position().to_vec();
    let camera = center - Vector2::new(x, y) * distance;
    let camera_altitude = (current * pixels_per_meter + distance * z) / pixels_per_meter;
    let dz = -pitch.cos();
    let above_ground = camera_altitude - elevation;
    let current_distance_meters = distance / pixels_per_meter;
    let min_safe_distance =
        (current_distance_meters * 0.25).max(DISTANCE_TO_CENTER_WHEN_LOOKING_UP_METERS);
    // Where the view ray looks down (`dz < -0.1`) but the new terrain at the center sits above
    // or nearly at the old camera altitude, sliding the center along the ray would collapse the
    // orbit distance to 10 m (zoom ~22). Move the camera up with the terrain instead, keeping
    // zoom, center and pitch, as `recalculate_globe_zoom_and_center` does.
    if dz < -0.1 && -above_ground / dz < min_safe_distance {
        view_state.set_center_elevation(elevation);
        return;
    }
    let (distance_meters, clamped_elevation) =
        distance_to_center_from_altitude(camera_altitude, elevation, pitch);
    let new_center = camera + Vector2::new(x, y) * (distance_meters * pixels_per_meter);
    let latitude = mercator_world_to_lat_lon(new_center.x, new_center.y, world_size).latitude;
    let new_world_size =
        distance / distance_meters * view_state.body().circumference_at_latitude(latitude);
    if !new_world_size.is_finite() || new_world_size <= 0.0 {
        return;
    }
    let new_zoom = (new_world_size / TILE_SIZE).log2();
    let position = new_center / world_size * new_world_size;
    view_state.set_center_elevation(clamped_elevation);
    view_state.update_zoom(Zoom::new(new_zoom));
    view_state.camera_mut().move_to(Point2::from_vec(position));
}

/// Metres around the parallel at a latitude, the length one Mercator unit covers there.
pub(super) fn circumference_at_latitude(body: Body, latitude_degrees: f64) -> f64 {
    body.circumference_at_latitude(latitude_degrees)
}

/// Grounds the map center and immediately lifts a constrained map camera above loaded terrain
/// once the center's own elevation is known (or explicitly pinned off-ground).
pub(crate) fn ground_and_clear_terrain(style: &Style, view_state: &mut ViewState, world: &World) {
    let grounded = crate::terrain::elevation::ground_center(view_state, world);
    let center_ready =
        grounded || !view_state.center_held_by_terrain() || view_state.center_elevation_frozen();
    if center_ready
        && view_state.camera_authority()
            == crate::render::view_state::CameraAuthority::ConstrainedMap
    {
        let _ = keep_camera_above_terrain(style, view_state, world);
    }
}

#[cfg(test)]
mod tests;
