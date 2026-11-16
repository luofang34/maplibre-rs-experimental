//! Pan-hole prevention fallbacks for non-terrain vector views: coarse `z - 3` ancestor cover,
//! active `PrefetchView` pan lookahead, and settled 1-ring edge prefetch within `MemoryBudget`.

use rustc_hash::FxHashSet;

use crate::{
    coords::{OverscaledTileId, WorldTileCoords, ZoomLevel},
    io::tile_sources::{outside_source_bounds, source_max_zoom, source_min_zoom, TileKind},
    render::{
        memory_budget::MemoryBudget,
        projection::view_region_for_projection,
        tile_view_pattern::{HasTile, DEFAULT_TILE_SIZE},
        view_state::{ViewState, ViewStatePadding},
    },
    style::Style,
    tcs::world::World,
    vector::upload_system::sources::VectorFallbackTiles,
};

/// Maximum coarse `z - 3` ancestor cover tiles requested alongside the tight viewport.
const MAX_COARSE_ANCESTORS: usize = 2;
/// Maximum `PrefetchView` pan lookahead tiles requested on `wasm32` during camera motion.
const MAX_PAN_LOOKAHEAD_TILES: usize = 6;
/// Maximum 1-ring edge tiles prefetched on `wasm32` after tight viewport tiles are uploaded.
const MAX_SETTLED_RING_TILES: usize = 8;

/// Computes extra non-terrain fallback/prefetch tiles (`wasm32` coarse cover, pan lookahead, and
/// post-upload 1-ring edge tiles) and records all non-tight retained canonical coordinates into
/// [`VectorFallbackTiles`] so `upload_system` uploads and keeps them in `VectorBufferPool`.
pub(super) fn plan_non_terrain_fallbacks(
    style: &Style,
    view_state: &ViewState,
    world: &mut World,
    (tight_requests, is_multiview): (bool, bool),
    view_tiles: &[WorldTileCoords],
    (level, magnifies): (u8, bool),
) -> Vec<WorldTileCoords> {
    if style.terrain.is_some() || tight_requests || is_multiview || view_tiles.is_empty() {
        world.resources.insert(VectorFallbackTiles::default());
        return Vec::new();
    }
    let memory = world
        .resources
        .get::<MemoryBudget>()
        .copied()
        .unwrap_or_default();
    if memory.is_tight() {
        world.resources.insert(VectorFallbackTiles::default());
        return Vec::new();
    }

    let visible_level = ZoomLevel::new(level);
    let view_set: FxHashSet<WorldTileCoords> = view_tiles.iter().copied().collect();
    let mut extra_requests = Vec::new();
    let mut seen_extra = view_set.clone();

    if cfg!(target_arch = "wasm32") && !view_state.has_external_view() {
        let max_zoom = source_max_zoom(style, TileKind::Vector);
        let source_z = max_zoom.map_or(level, |m| level.min(m));
        // 1. Coarse `z - 3` ancestor cover (1..=2 tiles covering 8x8 source tiles each) so
        // starting a pan or crossing into uncached columns never flashes raw background color.
        if source_z >= 3 {
            let coarse_z = source_z - 3;
            let mut coarse_count = 0;
            for &coords in view_tiles {
                let Some((wrapped, _)) = coords.wrapped() else {
                    continue;
                };
                let cur_z = u8::from(wrapped.z);
                if cur_z <= coarse_z {
                    continue;
                }
                let shift = u32::from(cur_z - coarse_z);
                let ancestor = WorldTileCoords {
                    x: wrapped.x >> shift,
                    y: wrapped.y >> shift,
                    z: ZoomLevel::new(coarse_z),
                };
                if seen_extra.insert(ancestor) {
                    extra_requests.push(ancestor);
                    coarse_count += 1;
                    if coarse_count >= MAX_COARSE_ANCESTORS {
                        break;
                    }
                }
            }
        }

        // 2. Active pan lookahead (`PrefetchView`) while the camera is moving.
        let ahead_view = world
            .resources
            .get::<crate::render::xr::PrefetchView>()
            .and_then(|pv| pv.view_state.clone());
        if let Some(ahead) = ahead_view {
            if let Ok(Some(region)) = view_region_for_projection(
                style,
                &ahead,
                world,
                visible_level,
                ViewStatePadding::Tight,
            ) {
                let mut lookahead_count = 0;
                for coords in region.iter() {
                    if seen_extra.insert(coords) {
                        extra_requests.push(coords);
                        lookahead_count += 1;
                        if lookahead_count >= MAX_PAN_LOOKAHEAD_TILES {
                            break;
                        }
                    }
                }
            }
        }

        // 3. Settled 1-ring edge prefetch once every tight viewport tile is already uploaded in
        // `VectorBufferPool` (runs after initial `waitArenaLoaded` settles, never delaying boot).
        let moving = crate::render::frame_signals::camera_moved(world, view_state);
        let in_flight = crate::io::tile_backpressure::tiles_in_flight(world);
        if !moving
            && in_flight == 0
            && all_tight_tiles_uploaded(world, style, view_tiles, level, magnifies)
        {
            if let Ok(Some(loose_region)) = view_region_for_projection(
                style,
                view_state,
                world,
                visible_level,
                ViewStatePadding::Loose,
            ) {
                let center = viewport_tile_centroid(view_tiles);
                let mut ring_candidates: Vec<WorldTileCoords> = loose_region
                    .iter()
                    .filter(|coords| !seen_extra.contains(coords))
                    .collect();
                ring_candidates
                    .sort_by(|a, b| tile_dist_sq(*a, center).total_cmp(&tile_dist_sq(*b, center)));
                for coords in ring_candidates.into_iter().take(MAX_SETTLED_RING_TILES) {
                    if seen_extra.insert(coords) {
                        extra_requests.push(coords);
                    }
                }
            }
        }
    }

    // Record canonical source coordinates for all retained non-tight tiles (both native `Loose`
    // ring tiles already in `view_tiles` and `wasm32` `extra_requests`) so `upload_system`
    // uploads and retains them in `VectorBufferPool`.
    let max_zoom = source_max_zoom(style, TileKind::Vector);
    let min_zoom = source_min_zoom(style, TileKind::Vector);
    let tight_set: FxHashSet<WorldTileCoords> = view_region_for_projection(
        style,
        view_state,
        world,
        view_state.zoom().zoom_level(DEFAULT_TILE_SIZE),
        ViewStatePadding::Tight,
    )
    .ok()
    .flatten()
    .map(|r| r.iter().collect())
    .unwrap_or_default();

    let mut fallback_canonical = Vec::new();
    let mut seen_canonical = FxHashSet::default();
    for &coords in view_tiles
        .iter()
        .filter(|c| !tight_set.contains(c))
        .chain(extra_requests.iter())
    {
        if min_zoom.is_some_and(|m| u8::from(coords.z) < m) {
            continue;
        }
        let target_level = u8::from(coords.z).min(level);
        let Some(target) = OverscaledTileId::for_source(coords, target_level, max_zoom, magnifies)
        else {
            continue;
        };
        let canonical = target.canonical;
        if !outside_source_bounds(style, TileKind::Vector, canonical)
            && seen_canonical.insert(canonical)
        {
            fallback_canonical.push(canonical);
        }
    }
    world
        .resources
        .insert(VectorFallbackTiles(fallback_canonical));

    extra_requests
}

fn all_tight_tiles_uploaded(
    world: &World,
    style: &Style,
    view_tiles: &[WorldTileCoords],
    level: u8,
    magnifies: bool,
) -> bool {
    let max_zoom = source_max_zoom(style, TileKind::Vector);
    let min_zoom = source_min_zoom(style, TileKind::Vector);
    let gpu = crate::vector::UploadedVectorGeometry::for_world(world);
    for &coords in view_tiles {
        if min_zoom.is_some_and(|m| u8::from(coords.z) < m) {
            continue;
        }
        let Some(target) = OverscaledTileId::for_source(coords, level, max_zoom, magnifies) else {
            continue;
        };
        let canonical = target.canonical;
        if outside_source_bounds(style, TileKind::Vector, canonical) {
            continue;
        }
        if !gpu.has_tile(canonical, world) {
            return false;
        }
    }
    true
}

fn viewport_tile_centroid(tiles: &[WorldTileCoords]) -> (f64, f64) {
    let count = tiles.len().max(1) as f64;
    let sum_x: f64 = tiles.iter().map(|t| f64::from(t.x)).sum();
    let sum_y: f64 = tiles.iter().map(|t| f64::from(t.y)).sum();
    (sum_x / count, sum_y / count)
}

fn tile_dist_sq(coords: WorldTileCoords, center: (f64, f64)) -> f64 {
    let dx = f64::from(coords.x) - center.0;
    let dy = f64::from(coords.y) - center.1;
    dx * dx + dy * dy
}
