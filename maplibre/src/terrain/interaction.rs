//! Terrain-aware camera interaction, following GL JS's handler manager and camera helpers.
//!
//! Gestures anchor on the terrain surface under the pointer rather than on the sea-level
//! plane, fall back to the view center while the pointer is in the sky, hold the center
//! elevation still while they run and reconcile it when they end, and the camera is lifted
//! out of the ground after every change.

use cgmath::{EuclideanSpace, InnerSpace, Point2, Vector2, Vector3};

use crate::{
    coords::{LatLon, Zoom, TILE_SIZE},
    projection::body::Body,
    projection::globe::{
        camera::GlobeCameraState, ray_sphere_intersection, unit_sphere_to_lat_lon,
    },
    render::{
        projection::{globe_camera_for_view, mercator_world_to_lat_lon},
        view_state::ViewState,
    },
    style::Style,
    tcs::{tiles::Tiles, world::World},
    terrain::coverage::{TerrainCoverageIndex, TerrainSample},
};

/// World pixels between ray samples on the flat map.
const TARGET_WORLD_STEP_PX: f64 = 4.0;
const MAX_SAMPLES: usize = 512;
const MERCATOR_BISECT_EPSILON_WORLD_PX: f64 = 1e-3;
const GLOBE_SAMPLES: usize = 256;
const GLOBE_BISECT_EPSILON_T: f64 = 1e-12;
const MAX_BISECTIONS: usize = 40;
const HIT_EPSILON_METERS: f64 = 1e-6;
/// An anchor this close to the camera's altitude above the center is unusable for a gesture.
const TERRAIN_ANCHOR_MAX_CAMERA_ALTITUDE_FRACTION: f64 = 0.9;
const MAX_VALID_LATITUDE: f64 = 85.051_128_779_806_59;
const MAX_MERCATOR_Y: f64 = 1.0 - 1e-9;
/// Distance assumed to the center when the camera looks away from the ground.
const DISTANCE_TO_CENTER_WHEN_LOOKING_UP_METERS: f64 = 10_000.0;

/// Where a screen ray meets the terrain surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainHit {
    /// Mercator coordinates in `0..1`.
    pub mercator: Point2<f64>,
    /// Exaggerated elevation in metres.
    pub elevation: f64,
}

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

/// Mercator coordinates in `0..1` of a geographic location.
fn lat_lon_to_mercator(location: LatLon) -> Point2<f64> {
    let x = location.longitude / 360.0 + 0.5;
    let y = (1.0 - location.latitude.to_radians().tan().asinh() / std::f64::consts::PI) * 0.5;
    Point2::new(x, y.clamp(0.0, MAX_MERCATOR_Y))
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
            return Some(TerrainHit {
                mercator: Point2::new(
                    (near.x + hit * delta.x) / world_size,
                    (near.y + hit * delta.y) / world_size,
                ),
                elevation: sample_at(hit).elevation,
            });
        }
        previous = t;
    }
    None
}

/// Casts the ray through a pixel of the globe against the rendered terrain.
pub fn screen_point_to_terrain_globe(
    camera: &GlobeCameraState,
    index: &TerrainCoverageIndex,
    tiles: &Tiles,
    pixel: Point2<f64>,
) -> Option<TerrainHit> {
    if index.is_empty() {
        return None;
    }
    let origin = camera.camera_position();
    let direction = camera.ray_direction_from_pixel(pixel)?;
    let outer = ray_sphere_intersection(
        origin,
        direction,
        camera.body().unit_radius_at(index.max_elevation()),
    )?;
    let inner = ray_sphere_intersection(
        origin,
        direction,
        camera.body().unit_radius_at(index.min_elevation()),
    );
    let t_start = outer.t_min.max(0.0);
    let t_end = inner.map_or(outer.t_max, |inner| inner.t_min);
    if t_end <= t_start {
        return None;
    }
    let sample_at = |t: f64| {
        let position = origin + direction * t;
        let radius = position.magnitude();
        let location = unit_sphere_to_lat_lon(position / radius);
        let mercator = lat_lon_to_mercator(location);
        let mut sample = index.sample(tiles, mercator.x, mercator.y);
        if location.latitude.abs() > MAX_VALID_LATITUDE {
            sample.elevation = 0.0;
        }
        (sample, radius, mercator)
    };
    let below = |t: f64| {
        let (sample, radius, _) = sample_at(t);
        is_below_terrain(sample, (radius - 1.0) * camera.body().radius_meters)
    };
    let mut previous = 0.0;
    for step in 0..=GLOBE_SAMPLES {
        let t = t_start + (t_end - t_start) * step as f64 / GLOBE_SAMPLES as f64;
        if below(t) {
            let (_, hi) = bisect(below, previous, t, GLOBE_BISECT_EPSILON_T);
            let (sample, _, mercator) = sample_at(hi);
            return Some(TerrainHit {
                mercator,
                elevation: sample.elevation,
            });
        }
        previous = t;
    }
    None
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
/// Computed from the center, pitch and bearing as GL JS `getCameraAltitude` does rather than
/// by inverting the view matrix, so a camera placed exactly on the terrain is not judged to
/// be inside it by a rounding error.
pub fn camera_ground_position(view_state: &ViewState) -> (Vector2<f64>, f64) {
    let pitch = view_state.camera().get_pitch().0;
    let bearing = view_state.camera().get_bearing().0;
    let distance = view_state.camera_to_center_distance();
    let (x, y, z) = camera_direction(pitch, bearing);
    let center = view_state.camera().position().to_vec();
    let position = center - Vector2::new(x, y) * distance;
    let altitude = z * distance / view_state.pixels_per_meter() + view_state.center_elevation();
    (position, altitude)
}

/// Unit direction from the center towards the camera, as GL JS `cameraDirectionFromPitchBearing`.
pub(crate) fn camera_direction(pitch: f64, bearing: f64) -> (f64, f64, f64) {
    let horizontal = pitch.sin();
    (
        horizontal * bearing.sin(),
        -horizontal * bearing.cos(),
        pitch.cos(),
    )
}

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

mod flat_gestures;
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
    if style.terrain.is_none() {
        return;
    }
    let Some(index) = world.resources.get::<TerrainCoverageIndex>() else {
        return;
    };
    let world_size = TILE_SIZE * 2_f64.powf(view_state.zoom().value());
    let center = view_state.camera().position();
    let sample = index.sample(&world.tiles, center.x / world_size, center.y / world_size);
    if sample.dem_loaded {
        recalculate_zoom_and_center(view_state, sample.elevation);
    }
}

/// Distance from a camera at `altitude` to the center at `elevation` along its pitch, and the
/// elevation the center ends up at when the camera looks away from the ground.
pub(crate) fn distance_to_center_from_altitude(
    altitude: f64,
    elevation: f64,
    pitch: f64,
) -> (f64, f64) {
    let dz = -pitch.cos();
    let above_ground = altitude - elevation;
    if dz * above_ground >= 0.0 || dz.abs() < 0.1 {
        let distance = DISTANCE_TO_CENTER_WHEN_LOOKING_UP_METERS;
        (distance, altitude + distance * dz)
    } else {
        (-above_ground / dz, elevation)
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

/// Lifts the camera above the terrain under it by raising the pitch and zooming out, keeping the
/// center in place, as GL JS `_elevateCameraIfInsideTerrain` does. Returns whether it moved.
pub fn keep_camera_above_terrain(style: &Style, view_state: &mut ViewState, world: &World) -> bool {
    if style.terrain.is_none() {
        return false;
    }
    let Some(index) = world.resources.get::<TerrainCoverageIndex>() else {
        return false;
    };
    let world_size = TILE_SIZE * 2_f64.powf(view_state.zoom().value());
    let (camera, altitude) = camera_ground_position(view_state);
    let zoom = u8::from(view_state.zoom().zoom_level(TILE_SIZE));
    let Some(min_altitude) = index.elevation_at_zoom(
        &world.tiles,
        camera.x / world_size,
        camera.y / world_size,
        zoom,
    ) else {
        return false;
    };
    if altitude >= min_altitude {
        return false;
    }
    tracing::debug!(
        altitude,
        min_altitude,
        zoom = view_state.zoom().value(),
        pitch = view_state.camera().get_pitch().0.to_degrees(),
        center_elevation = view_state.center_elevation(),
        "camera inside terrain; lifting it"
    );
    let center = view_state.camera().position().to_vec();
    let center_elevation = view_state.center_elevation();
    // Both altitudes use the Mercator scale at the center, the scale the camera altitude is
    // measured with, so the lifted camera lands on the surface rather than a few metres off.
    let circumference = circumference_at_latitude(
        view_state.body(),
        mercator_world_to_lat_lon(center.x, center.y, world_size).latitude,
    );
    let from = Vector3::new(
        camera.x / world_size,
        camera.y / world_size,
        min_altitude / circumference,
    );
    let to = Vector3::new(
        center.x / world_size,
        center.y / world_size,
        center_elevation / circumference,
    );
    let delta = to - from;
    let distance_3d = delta.magnitude();
    if distance_3d <= f64::EPSILON {
        return false;
    }
    let ground = delta.x.hypot(delta.y);
    let zoom = (view_state.camera_to_center_distance() / distance_3d / TILE_SIZE).log2();
    let mut pitch = (ground / distance_3d).clamp(-1.0, 1.0).acos();
    pitch = if delta.z < 0.0 {
        std::f64::consts::FRAC_PI_2 - pitch
    } else {
        std::f64::consts::FRAC_PI_2 + pitch
    };
    view_state.camera_mut().set_pitch(cgmath::Rad(pitch));
    view_state.zoom_to(Zoom::new(zoom));
    true
}

/// Metres around the parallel at a latitude, the length one Mercator unit covers there.
fn circumference_at_latitude(body: Body, latitude_degrees: f64) -> f64 {
    body.circumference_at_latitude(latitude_degrees)
}

#[cfg(test)]
mod tests;
