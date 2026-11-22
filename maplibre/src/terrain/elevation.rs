//! The height of the point the camera orbits at the map center, from the loaded DEM tiles.

use std::time::Duration;

use crate::{
    context::MapContext,
    coords::{LatLon, WorldCoords, WorldTileCoords, Zoom, ZoomLevel, TILE_SIZE},
    render::{
        frame_input::FrameInput,
        view_state::{CameraAuthority, NavigationMode, ViewState},
    },
    style::Style,
    tcs::{system::SystemResult, world::World},
    terrain::coverage::TerrainCoverageIndex,
};

/// Tracks coarse-to-fine DEM refinements under a stationary camera so `center_elevation` glides
/// smoothly instead of snapping by ~1,000 m in a single frame when valley DEM tiles replace
/// coarse continental ancestors.
#[derive(Clone, Copy, Debug, Default)]
struct CenterDemSmoothing {
    last_mercator: [f64; 2],
    last_zoom: f64,
    last_dem_zoom: Option<u8>,
    last_timestamp: Option<Duration>,
    smoothing_active: bool,
    initialized: bool,
}

/// Whether the globe camera of `style` orbits the center's elevation.
///
/// Pure vertical perspective has no flat camera to fall back to, so its globe camera orbits
/// the point the camera looks at, on the terrain or at the center altitude, as the flat
/// camera does; the globe preset keeps GL JS's sea-level orbit.
pub(crate) fn globe_orbits_center(style: &crate::style::Style) -> bool {
    style.projection.as_ref().is_some_and(|projection| {
        projection.projection_type == crate::projection::ProjectionType::VerticalPerspective
            || (projection.projection_type == crate::projection::ProjectionType::Globe
                && style.center_altitude.is_some())
    })
}

/// Decides what the camera orbits at the center every frame, terrain or not: whether the globe
/// camera orbits the center's elevation, and, without terrain or with the center off the
/// ground, rests the center at its altitude. A gesture holds it where it is.
pub fn center_target_system(
    MapContext {
        style, view_state, ..
    }: &mut MapContext,
) -> SystemResult {
    view_state.set_globe_orbits_center(globe_orbits_center(style));
    view_state.set_center_over_terrain(style.terrain.is_some());
    let projection = style
        .projection
        .as_ref()
        .map_or_else(Default::default, |projection| {
            projection.projection_type.clone()
        });
    view_state.enforce_navigation(&projection);
    let off_ground = style.terrain.is_none() || !view_state.center_clamped_to_ground();
    if off_ground && !view_state.center_elevation_frozen() {
        view_state.set_center_elevation(view_state.center_altitude());
    }
    if style.terrain.is_none() {
        view_state.set_min_elevation(0.0);
    }
    Ok(())
}

/// Lifts the camera's orbit point onto the terrain under the map center every frame, records
/// the lowest elevation of the tile under it for the far plane, and enforces eye/near-plane
/// terrain clearance for constrained map cameras on boot, async DEM load, and jumps.
pub fn center_elevation_system(
    MapContext {
        style,
        view_state,
        world,
        ..
    }: &mut MapContext,
) -> SystemResult {
    // Off the ground the center rests at its altitude, which center_target_system applies.
    if style.terrain.is_none() {
        return Ok(());
    }
    let now_ts = world.resources.get::<FrameInput>().map(|f| f.timestamp);
    let center_ready = ground_center_smoothed(view_state, world, now_ts);
    if !past_mercator_rows(view_state) {
        if let Some(index) = world.resources.get::<TerrainCoverageIndex>() {
            let zoom = view_state.zoom();
            let center = view_state.camera().position();
            let center_tile = WorldCoords::at_ground(center.x, center.y)
                .into_world_tile(zoom.zoom_level(TILE_SIZE), zoom);
            if let Some(range) = index.tile_elevation_range(center_tile) {
                view_state.set_min_elevation(range.min_meters);
            }
        }
    }
    if center_ready
        && view_state.camera_authority() == CameraAuthority::ConstrainedMap
        && crate::terrain::interaction::keep_camera_above_terrain(style, view_state, world)
    {
        if let Some(state) = world.resources.get_mut::<CenterDemSmoothing>() {
            state.last_zoom = view_state.zoom().value();
        }
    }
    Ok(())
}

/// Grounds the map center and immediately lifts a constrained map camera above loaded terrain
/// once the center's own elevation is known (or explicitly pinned off-ground).
pub(crate) fn ground_and_clear_terrain(style: &Style, view_state: &mut ViewState, world: &World) {
    let grounded = ground_center(view_state, world);
    let center_ready =
        grounded || !view_state.center_held_by_terrain() || view_state.center_elevation_frozen();
    if center_ready && view_state.camera_authority() == CameraAuthority::ConstrainedMap {
        let _ = crate::terrain::interaction::keep_camera_above_terrain(style, view_state, world);
    }
}

fn ground_center_smoothed(
    view_state: &mut ViewState,
    world: &mut World,
    now_ts: Option<Duration>,
) -> bool {
    let zoom_val = view_state.zoom().value();
    let world_size = TILE_SIZE * 2_f64.powf(zoom_val);
    let pos = view_state.camera().position();
    let mercator = [pos.x / world_size.max(1.0), pos.y / world_size.max(1.0)];

    if !view_state.center_held_by_terrain() || view_state.center_elevation_frozen() {
        let state = world.resources.get_or_init_mut::<CenterDemSmoothing>();
        state.last_mercator = mercator;
        state.last_zoom = zoom_val;
        state.last_timestamp = now_ts;
        state.smoothing_active = false;
        return true;
    }
    let Some((target, dem_zoom)) = center_ground_with_dem_zoom(view_state, world) else {
        return false;
    };
    if view_state.navigation_mode() == NavigationMode::FreeGlobe {
        let _ = view_state.ground_free_globe_target(target);
        let state = world.resources.get_or_init_mut::<CenterDemSmoothing>();
        state.last_mercator = mercator;
        state.last_zoom = zoom_val;
        state.last_dem_zoom = dem_zoom;
        state.last_timestamp = now_ts;
        state.smoothing_active = false;
        state.initialized = true;
        return true;
    }

    let keep_anim = {
        let state = world.resources.get_or_init_mut::<CenterDemSmoothing>();
        step_center_smoothing(
            view_state,
            state,
            SmoothingSample {
                target,
                dem_zoom,
                mercator,
                zoom_val,
                world_size,
                now_ts,
            },
        )
    };
    if keep_anim {
        crate::render::frame_signals::keep_animating(world);
    }
    true
}

struct SmoothingSample {
    target: f64,
    dem_zoom: Option<u8>,
    mercator: [f64; 2],
    zoom_val: f64,
    world_size: f64,
    now_ts: Option<Duration>,
}

fn step_center_smoothing(
    view_state: &mut ViewState,
    state: &mut CenterDemSmoothing,
    sample: SmoothingSample,
) -> bool {
    let SmoothingSample {
        target,
        dem_zoom,
        mercator,
        zoom_val,
        world_size,
        now_ts,
    } = sample;
    let current = view_state.center_elevation();
    let dt = match (state.last_timestamp, now_ts) {
        (Some(prev), Some(now)) if now > prev => now - prev,
        _ => Duration::ZERO,
    };
    let [mx, my] = mercator;
    let [lx, ly] = state.last_mercator;
    let moved_px = (mx - lx).hypot(my - ly) * world_size;
    let camera_moved =
        !state.initialized || moved_px > 0.5 || (zoom_val - state.last_zoom).abs() > 1e-3;
    let dem_refined_only = !camera_moved
        && state.last_dem_zoom.is_some()
        && state.last_dem_zoom != dem_zoom
        && (target - current).abs() > 1.0;

    if camera_moved || dt.is_zero() || dt > Duration::from_millis(250) {
        state.smoothing_active = false;
    } else if dem_refined_only {
        state.smoothing_active = true;
    }

    let mut keep_anim = false;
    if state.smoothing_active {
        let alpha = (1.0 - (-dt.as_secs_f64() * 14.0).exp()).clamp(0.10, 1.0);
        let next = current + (target - current) * alpha;
        if (target - next).abs() <= 0.2 {
            view_state.set_center_elevation(target);
            state.smoothing_active = false;
        } else {
            view_state.set_center_elevation(next);
            keep_anim = true;
        }
    } else {
        view_state.set_center_elevation(target);
    }

    state.last_mercator = mercator;
    state.last_zoom = zoom_val;
    state.last_dem_zoom = dem_zoom;
    state.last_timestamp = now_ts;
    state.initialized = true;
    keep_anim
}

/// Stands the center on the loaded ground under it, where terrain holds the center and no
/// gesture freezes it, as the frame does; a free-globe pose whose eye that would carry onto
/// or under the ground keeps its target in the air. A host placing the camera between frames
/// calls this so what it reads back already stands on the ground, as GL JS `jumpTo` grounds
/// its center.
/// Returns whether the center now stands on the ground; where the ground under it is not
/// loaded, its elevation stays as it was.
pub fn ground_center(view_state: &mut ViewState, world: &World) -> bool {
    if !view_state.center_held_by_terrain() || view_state.center_elevation_frozen() {
        return false;
    }
    let Some(elevation) = center_ground(view_state, world) else {
        return false;
    };
    // A free-globe pose looking up from near the ground keeps its target in the air.
    if view_state.navigation_mode() == crate::render::view_state::NavigationMode::FreeGlobe {
        return view_state.ground_free_globe_target(elevation);
    }
    view_state.set_center_elevation(elevation);
    true
}

/// The loaded ground under the map center the center stands on, or `None` where it is not
/// loaded: the DEM tile at the integer zoom, as GL JS lifts its center by, not the tile drawn
/// under it, which a coarser level may stand in for at a distance.
pub(crate) fn center_ground(view_state: &ViewState, world: &World) -> Option<f64> {
    center_ground_with_dem_zoom(view_state, world).map(|(elev, _)| elev)
}

fn center_ground_with_dem_zoom(view_state: &ViewState, world: &World) -> Option<(f64, Option<u8>)> {
    if let Some(pose) = view_state.pose_view() {
        if pose.center.latitude.abs() > crate::projection::globe::scale::MERCATOR_LATITUDE_LIMIT {
            let index = world.resources.get::<TerrainCoverageIndex>()?;
            let poles = world
                .resources
                .get::<crate::terrain::PolarPoleElevations>()
                .copied()
                .unwrap_or_else(|| index.poles());
            let meters = if pose.center.latitude > 0.0 {
                poles.north_meters
            } else {
                poles.south_meters
            };
            return Some((f64::from(meters) * index.exaggeration(), None));
        }
    }
    let zoom = view_state.zoom();
    let world_size = TILE_SIZE * 2_f64.powf(zoom.value());
    let center = view_state.camera().position();
    ground_at_mercator_with_dem_zoom(
        world,
        center.x / world_size,
        center.y / world_size,
        zoom.value(),
    )
}

/// The loaded ground a center at `position` and `zoom` stands on, as [`center_ground`] reads
/// it; past the Mercator world's last row the cap closes the globe at the configured pole datum.
pub(crate) fn ground_under(world: &World, position: LatLon, zoom: f64) -> Option<f64> {
    if position.latitude.abs() > crate::projection::globe::scale::MERCATOR_LATITUDE_LIMIT {
        let index = world.resources.get::<TerrainCoverageIndex>();
        let poles = world
            .resources
            .get::<crate::terrain::PolarPoleElevations>()
            .copied()
            .or_else(|| index.map(TerrainCoverageIndex::poles))
            .unwrap_or_default();
        let meters = if position.latitude > 0.0 {
            poles.north_meters
        } else {
            poles.south_meters
        };
        let exag = index.map_or(1.0, |idx| {
            if idx.exaggeration() > 0.0 {
                idx.exaggeration()
            } else {
                1.0
            }
        });
        return Some(f64::from(meters) * exag);
    }
    let mercator = WorldCoords::from_lat_lon(position, Zoom::new(0.0));
    ground_at_mercator(world, mercator.x / TILE_SIZE, mercator.y / TILE_SIZE, zoom)
}

fn ground_at_mercator(world: &World, x: f64, y: f64, zoom: f64) -> Option<f64> {
    ground_at_mercator_with_dem_zoom(world, x, y, zoom).map(|(elev, _)| elev)
}

fn ground_at_mercator_with_dem_zoom(
    world: &World,
    x: f64,
    y: f64,
    zoom: f64,
) -> Option<(f64, Option<u8>)> {
    let index = world.resources.get::<TerrainCoverageIndex>()?;
    let tile_zoom = zoom.floor().clamp(0.0, f64::from(u8::MAX)) as u8;
    let elev = index.loaded_ground_cached(
        &world.tiles,
        x,
        y,
        crate::terrain::DemDetail::Tile(tile_zoom),
    )?;
    let scale = 2_f64.powi(i32::from(tile_zoom));
    let wrapped_x = x - x.floor();
    let coords = WorldTileCoords {
        x: (wrapped_x * scale).floor() as i32,
        y: (y.clamp(0.0, 1.0 - f64::EPSILON) * scale).floor() as i32,
        z: ZoomLevel::new(tile_zoom),
    };
    let dem_zoom = index.loaded_dem_for(coords).map(|dem| u8::from(dem.z));
    Some((elev, dem_zoom))
}

/// Whether a free-globe camera looks past the last row of tiles, where the cap closes the
/// globe at sea level; the flat center stops at that row.
fn past_mercator_rows(view_state: &ViewState) -> bool {
    view_state.pose_view().is_some_and(|pose| {
        pose.center.latitude.abs() > crate::projection::globe::scale::MERCATOR_LATITUDE_LIMIT
    })
}

#[cfg(test)]
mod tests;
