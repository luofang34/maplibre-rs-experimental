//! Evicts tiles that left the view, keeping a recently used cache sized from the viewport.
//!
//! The tile store would otherwise grow for the whole session. GL JS keeps the tiles its
//! covering needs plus a cache of `MAX_TILE_CACHE_ZOOM_LEVELS` times the tiles in view and
//! unloads the rest; this system does the same once per frame in the Cleanup stage. A tile
//! whose worker request is still in flight is never evicted, so its result cannot land on a
//! re-requested tile and duplicate its layers. Tiles the frame requests but does not draw,
//! such as those around an external eye, count as in use too: evicting one would only have
//! the request systems fetch it again on the next frame. Only drawn tiles size the cache,
//! as GL JS sizes it from the viewport; a wide request would otherwise keep every tile a
//! flight passes resident.

use std::{collections::HashSet, hash::BuildHasher};

use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    context::MapContext,
    coords::WorldTileCoords,
    io::{
        tile_backpressure::is_settled,
        tile_sources::{source_max_zoom, TileKind},
    },
    render::{
        eventually::{Eventually, Eventually::Initialized},
        memory_budget::{MemoryBudget, MemoryPressure},
        projection::view_region_for_projection,
        tile_memory::tile_bytes,
        tile_view_pattern::{WgpuTileViewPattern, DEFAULT_TILE_SIZE},
        view_state::{ViewState, ViewStatePadding},
    },
    style::Style,
    tcs::{system::SystemResult, world::World},
    vector::VectorBufferPool,
};

mod ancestors;
mod gpu;
mod summary;

pub(crate) use ancestors::finish_tiles_in_use;

/// Out-of-view tiles kept per tile in use.
const CACHE_TILES_PER_VIEW_TILE: usize = 2;
/// Smallest cache, so a tiny viewport still keeps some history for panning back.
#[cfg(target_arch = "wasm32")]
const MIN_CACHE_TILES: usize = 2;
#[cfg(not(target_arch = "wasm32"))]
const MIN_CACHE_TILES: usize = 2;
/// Hard ceiling on out-of-view cached tiles so wide oblique/globe coverings cannot accumulate
/// hundreds of multi-megabyte raster/DEM tiles along a corridor.
#[cfg(target_arch = "wasm32")]
const MAX_CACHE_TILES: usize = 2;
#[cfg(not(target_arch = "wasm32"))]
const MAX_CACHE_TILES: usize = 4;
/// Tessellated geometry the out-of-view cache may hold.
#[cfg(target_arch = "wasm32")]
const CACHE_BYTES: usize = 1 << 20;
#[cfg(not(target_arch = "wasm32"))]
const CACHE_BYTES: usize = 2 << 20;
/// Frames between resident-memory summaries for hosts driving the map from an eye, which
/// run on devices with a memory limit.
const SUMMARY_EVERY_FRAMES: u64 = 90;

/// What the out-of-view cache may hold.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CacheBudget {
    pub tiles: usize,
    pub bytes: usize,
}

/// Last frame each tile was needed by the view pattern.
#[derive(Default)]
pub struct TileRetention {
    last_used: FxHashMap<WorldTileCoords, u64>,
    frame: u64,
}

/// Offline hosts cannot replace an evicted tile without an active source loader.
pub(crate) struct RetainLoadedTiles;

fn run_eviction(
    world: &mut World,
    style: &Style,
    view_state: &ViewState,
    queue: Option<&crate::render::upload_queue::UploadQueue>,
    cancel_unwanted: bool,
    include_drawn: bool,
    tight_tile_requests: bool,
) -> (FxHashSet<WorldTileCoords>, usize) {
    let floor = world
        .resources
        .get::<crate::render::eye_covering::SlotResidencyFloor>()
        .cloned();
    let multi_active = world
        .resources
        .get::<crate::render::eye_covering::MultiViewExtractCovering>()
        .is_some_and(|m| m.active);
    let (mut in_use, drawn) = if multi_active {
        let drawn_set = drawn_tiles(world);
        let drawn = drawn_set
            .len()
            .max(floor.as_ref().map_or(0, |floor| floor.drawn_count));
        let mut combined = drawn_set;
        if let Some(floor) = &floor {
            combined.extend(floor.retained_tiles.iter().copied());
            combined.extend(floor.retained_drapes.iter().copied());
        }
        (finish_tiles_in_use(world, style, combined), drawn)
    } else {
        let mut single =
            tiles_in_use_inner(world, style, view_state, include_drawn, tight_tile_requests);
        if let Some(floor) = &floor {
            single.extend(floor.retained_tiles.iter().copied());
            single.extend(floor.retained_drapes.iter().copied());
        }
        let drawn = if include_drawn {
            drawn_tiles(world)
                .len()
                .max(floor.as_ref().map_or(0, |floor| floor.drawn_count))
        } else {
            single
                .len()
                .max(floor.as_ref().map_or(0, |floor| floor.drawn_count))
        };
        (single, drawn)
    };
    let _ = &mut in_use;
    if cancel_unwanted {
        drop_cancelled(world, &in_use, queue);
    }
    let evicted = evict_stale_tiles(world, &in_use, drawn);
    if !evicted.is_empty() {
        tracing::debug!(count = evicted.len(), "evicted tiles that left the view");
        gpu::drop_gpu_data_with_queue(world, &evicted, queue);
    }
    gpu::reclaim_pool_pages(world, queue);
    (in_use, drawn)
}

/// Frees unneeded out-of-view GPU buffers right before uploading new vector buckets.
pub(crate) fn evict_before_upload(
    world: &mut World,
    style: &Style,
    view_state: &ViewState,
    queue: &crate::render::upload_queue::UploadQueue,
    tight_tile_requests: bool,
) {
    let _ = run_eviction(
        world,
        style,
        view_state,
        Some(queue),
        false,
        false,
        tight_tile_requests,
    );
}

/// Drops the least recently used tiles beyond the cache budget and releases their GPU data.
pub fn retention_system(
    MapContext {
        world,
        style,
        view_state,
        renderer,
        ..
    }: &mut MapContext,
) -> SystemResult {
    if crate::render::eye_covering::EyeInFrame::reuses_content(world) {
        return Ok(());
    }
    let (in_use, drawn) = run_eviction(
        world,
        style,
        view_state,
        Some(&renderer.queue),
        true,
        true,
        renderer.settings.tight_tile_requests,
    );
    if view_state.has_external_view() {
        summary::summarize_residency(world, drawn, in_use.len(), view_state.eye_settled());
        #[cfg(not(target_arch = "wasm32"))]
        summary::summarize_gpu_objects(world, renderer);
        summary::summarize_timings(world);
    }
    Ok(())
}

/// The tiles the frame draws from, with the tiles whose shapes stand in for them.
pub(crate) fn drawn_tiles(world: &World) -> FxHashSet<WorldTileCoords> {
    let mut drawn = FxHashSet::default();
    if let Some(Initialized(pattern)) = world.resources.get::<Eventually<WgpuTileViewPattern>>() {
        for view_tile in pattern.iter() {
            drawn.insert(view_tile.coords());
            view_tile.render(|shape| {
                drawn.insert(shape.coords());
            });
        }
    }
    if let Some(covering) = world
        .resources
        .get::<crate::sdf::covering::SymbolCovering>()
    {
        drawn.extend(covering.tiles.iter().copied());
    }
    drawn
}

/// Builds the retained tile set from already-computed `Loose` view and raster coverings.
#[cfg(feature = "headless")]
pub(crate) fn tiles_in_use_from_loose(
    world: &World,
    style: &Style,
    loose_view: impl IntoIterator<Item = WorldTileCoords>,
    loose_raster: impl IntoIterator<Item = WorldTileCoords>,
) -> FxHashSet<WorldTileCoords> {
    let mut in_use = drawn_tiles(world);
    let max_zoom = source_max_zoom(style, TileKind::Vector);
    for coords in loose_view {
        retain_loose_vector_tile(&mut in_use, coords, max_zoom);
    }
    in_use.extend(loose_raster);
    finish_tiles_in_use(world, style, in_use)
}

fn retain_loose_vector_tile(
    in_use: &mut FxHashSet<WorldTileCoords>,
    coords: WorldTileCoords,
    max_zoom: Option<u8>,
) {
    if let Some((canonical, _)) = coords.wrapped() {
        in_use.insert(canonical);
    } else {
        in_use.insert(coords);
    }
    if let Some(target) =
        crate::coords::OverscaledTileId::for_source(coords, u8::from(coords.z), max_zoom, false)
    {
        in_use.insert(target.canonical);
    }
}

/// Every tile the current frame draws from or requests, with the DEM tiles and ancestors
/// terrain reads.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn tiles_in_use(
    world: &World,
    style: &Style,
    view_state: &ViewState,
) -> FxHashSet<WorldTileCoords> {
    tiles_in_use_inner(world, style, view_state, true, false)
}

fn tiles_in_use_inner(
    world: &World,
    style: &Style,
    view_state: &ViewState,
    include_drawn: bool,
    tight_tile_requests: bool,
) -> FxHashSet<WorldTileCoords> {
    let mut in_use = if include_drawn {
        drawn_tiles(world)
    } else {
        FxHashSet::default()
    };
    let host_frame = crate::render::frame_signals::host_frame_in_progress(world);
    let loose_cache = world
        .resources
        .get::<crate::render::eye_covering::FrameLooseCovering>()
        .filter(|cache| cache.host_frame == host_frame);

    let request_padding = if tight_tile_requests
        || (cfg!(target_arch = "wasm32") && !view_state.has_external_view())
    {
        ViewStatePadding::Tight
    } else {
        ViewStatePadding::Loose
    };
    let max_zoom = source_max_zoom(style, TileKind::Vector);
    if let Some((cached_view, _, _)) = loose_cache
        .and_then(|c| c.view_tiles.as_ref())
        .filter(|(_, padding, _)| *padding == request_padding)
    {
        for &coords in cached_view.iter().flatten() {
            retain_loose_vector_tile(&mut in_use, coords, max_zoom);
        }
    } else {
        match view_region_for_projection(
            style,
            view_state,
            world,
            view_state.zoom().zoom_level(DEFAULT_TILE_SIZE),
            request_padding,
        ) {
            Ok(Some(requested)) => {
                for coords in requested.iter() {
                    retain_loose_vector_tile(&mut in_use, coords, max_zoom);
                }
            }
            Ok(None) => {}
            Err(error) => tracing::warn!(%error, "cannot select the requested tiles to keep"),
        }
    }

    if let Some((cached_raster, _)) = loose_cache
        .and_then(|c| c.raster_tiles.as_ref())
        .filter(|(_, padding)| *padding == request_padding)
    {
        in_use.extend(cached_raster.iter().copied());
    } else if let Ok(regions) =
        crate::render::projection::raster_source_regions(style, view_state, world, request_padding)
    {
        for (_, tiles) in regions {
            in_use.extend(tiles);
        }
    }
    ancestors::retain_terrain_region_if_needed(
        world,
        style,
        view_state,
        request_padding,
        &mut in_use,
    );
    finish_tiles_in_use(world, style, in_use)
}

/// Returns the out-of-view cache budget under current memory pressure, honoring `RetainLoadedTiles`.
fn effective_cache_budget(world: &World, view_tiles: usize) -> CacheBudget {
    if world.resources.get::<RetainLoadedTiles>().is_some() {
        return CacheBudget {
            tiles: usize::MAX,
            bytes: usize::MAX,
        };
    }
    let pressure = world
        .resources
        .get::<MemoryBudget>()
        .map_or(MemoryPressure::Comfortable, MemoryBudget::pressure);
    match pressure {
        MemoryPressure::Critical => CacheBudget { tiles: 0, bytes: 0 },
        MemoryPressure::Low => CacheBudget {
            tiles: MIN_CACHE_TILES,
            bytes: CACHE_BYTES / 2,
        },
        MemoryPressure::Comfortable => CacheBudget {
            tiles: (view_tiles * CACHE_TILES_PER_VIEW_TILE).clamp(MIN_CACHE_TILES, MAX_CACHE_TILES),
            bytes: CACHE_BYTES,
        },
    }
}

/// Removes the least recently used settled tiles beyond the cache budget and returns them;
/// the tile budget follows `view_tiles`, the tiles the frame draws.
pub(crate) fn evict_stale_tiles<S: BuildHasher>(
    world: &mut World,
    in_use: &HashSet<WorldTileCoords, S>,
    view_tiles: usize,
) -> Vec<WorldTileCoords> {
    evict_beyond(world, in_use, effective_cache_budget(world, view_tiles))
}

/// Removes the least recently used settled tiles until the rest fit `budget`, in tiles and
/// in bytes of geometry, and returns them.
pub(crate) fn evict_beyond<S: BuildHasher>(
    world: &mut World,
    in_use: &HashSet<WorldTileCoords, S>,
    budget: CacheBudget,
) -> Vec<WorldTileCoords> {
    let World { resources, tiles } = world;
    {
        let retention = resources.get_or_init_mut::<TileRetention>();
        retention.frame = retention.frame.wrapping_add(1);
        let frame = retention.frame;
        for coords in in_use {
            retention.last_used.insert(*coords, frame);
        }
    }

    let last_used = resources.get::<TileRetention>();
    let vector_pool = match resources.get::<Eventually<VectorBufferPool>>() {
        Some(Initialized(pool)) => Some(pool),
        _ => None,
    };
    let symbol_pool = match resources.get::<Eventually<crate::sdf::SymbolBufferPool>>() {
        Some(Initialized(pool)) => Some(pool),
        _ => None,
    };
    let mut candidates: Vec<(u64, WorldTileCoords, usize)> = tiles
        .tiles
        .values()
        .map(|tile| tile.coords)
        .filter(|coords| !in_use.contains(coords) && is_settled(tiles, *coords))
        .map(|coords| {
            let gpu_bytes = vector_pool.map_or(0, |p| p.index().tile_gpu_bytes(coords))
                + symbol_pool.map_or(0, |p| p.index().tile_gpu_bytes(coords));
            (
                last_used
                    .and_then(|r| r.last_used.get(&coords).copied())
                    .unwrap_or(0),
                coords,
                tile_bytes(tiles, coords).saturating_add(gpu_bytes),
            )
        })
        .collect();
    let mut kept = candidates.len();
    let mut kept_bytes: usize = candidates.iter().map(|(_, _, bytes)| bytes).sum();
    if kept <= budget.tiles && kept_bytes <= budget.bytes {
        return Vec::new();
    }
    candidates.sort_by_key(|(used, _, _)| *used);
    let mut evicted = Vec::new();
    for (_, coords, bytes) in candidates {
        if kept <= budget.tiles && kept_bytes <= budget.bytes {
            break;
        }
        kept -= 1;
        kept_bytes -= bytes;
        evicted.push(coords);
    }
    if !evicted.is_empty() {
        let retention = resources.get_or_init_mut::<TileRetention>();
        for coords in &evicted {
            tiles.remove(*coords);
            retention.last_used.remove(coords);
        }
    }
    evicted
}

/// Drops the tiles whose requests no one wants any more, freeing their slots for tiles in view.
fn drop_cancelled<S: BuildHasher>(
    world: &mut World,
    in_use: &HashSet<WorldTileCoords, S>,
    queue: Option<&crate::render::upload_queue::UploadQueue>,
) {
    let cancelled = crate::io::tile_retry::cancel_unwanted(world, in_use);
    if cancelled.is_empty() {
        return;
    }
    tracing::debug!(
        count = cancelled.len(),
        "cancelled requests for tiles out of view"
    );
    let retention = world.resources.get_or_init_mut::<TileRetention>();
    for coords in &cancelled {
        retention.last_used.remove(coords);
    }
    for coords in &cancelled {
        world.tiles.remove(*coords);
    }
    gpu::drop_gpu_data_with_queue(world, &cancelled, queue);
}

#[cfg(test)]
mod tests;
