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
        tile_view_pattern::{WgpuTileViewPattern, DEFAULT_TILE_SIZE},
        view_state::{ViewState, ViewStatePadding},
    },
    style::Style,
    tcs::{system::SystemResult, world::World},
    vector::VectorBufferPool,
};

mod ancestors;
mod bytes_cache;
mod gpu;
mod summary;

pub(crate) use ancestors::finish_tiles_in_use;

/// Out-of-view tiles kept per tile in use.
const CACHE_TILES_PER_VIEW_TILE: usize = 2;
/// Smallest cache, so a tiny viewport still keeps some history for panning back.
#[cfg(target_arch = "wasm32")]
const MIN_CACHE_TILES: usize = 4;
#[cfg(not(target_arch = "wasm32"))]
const MIN_CACHE_TILES: usize = 8;
/// Hard ceiling on out-of-view cached tiles so wide oblique/globe coverings cannot accumulate
/// hundreds of multi-megabyte raster/DEM tiles along a corridor.
#[cfg(target_arch = "wasm32")]
const MAX_CACHE_TILES: usize = 10;
#[cfg(not(target_arch = "wasm32"))]
const MAX_CACHE_TILES: usize = 24;
/// Tessellated geometry the out-of-view cache may hold.
#[cfg(target_arch = "wasm32")]
const CACHE_BYTES: usize = 10 << 20;
#[cfg(not(target_arch = "wasm32"))]
const CACHE_BYTES: usize = 12 << 20;
/// Frames between resident-memory summaries for hosts driving the map from an eye, which
/// run on devices with a memory limit.
const SUMMARY_EVERY_FRAMES: u64 = 90;

/// What the out-of-view cache may hold.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CacheBudget {
    pub tiles: usize,
    pub bytes: usize,
}

struct BeforeUploadState {
    host_frame: u64,
    arrival_epoch: u64,
    tiles_len: usize,
    in_use: FxHashSet<WorldTileCoords>,
    drawn: usize,
}

/// Last frame each tile was needed by the view pattern.
#[derive(Default)]
pub struct TileRetention {
    last_used: FxHashMap<WorldTileCoords, u64>,
    bytes_cache: bytes_cache::TileBytesCache,
    frame: u64,
    before_upload: Option<BeforeUploadState>,
}

/// Offline hosts cannot replace an evicted tile without an active source loader.
pub(crate) struct RetainLoadedTiles;

fn run_eviction(
    world: &mut World,
    style: &Style,
    view_state: &ViewState,
    queue: Option<&crate::render::upload_queue::UploadQueue>,
    (cancel_unwanted, include_drawn, tight_tile_requests): (bool, bool, bool),
    extra_in_use: &[WorldTileCoords],
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
    in_use.extend(extra_in_use.iter().copied());
    if cancel_unwanted {
        drop_cancelled(world, &in_use, queue);
    }
    let evicted = evict_stale_tiles(world, &in_use, drawn);
    if !evicted.is_empty() {
        tracing::debug!(count = evicted.len(), "evicted tiles that left the view");
        ancestors::bump_arrival_epochs(world);
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
    extra_in_use: &[WorldTileCoords],
) {
    let (in_use, drawn) = run_eviction(
        world,
        style,
        view_state,
        Some(queue),
        (false, true, tight_tile_requests),
        extra_in_use,
    );
    let host_frame = crate::render::frame_signals::host_frame_in_progress(world);
    if host_frame > 0 {
        let arrival_epoch = crate::vector::arrival_epoch(world);
        let tiles_len = world.tiles.tiles.len();
        world
            .resources
            .get_or_init_mut::<TileRetention>()
            .before_upload = Some(BeforeUploadState {
            host_frame,
            arrival_epoch,
            tiles_len,
            in_use,
            drawn,
        });
    }
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
    let host_frame = crate::render::frame_signals::host_frame_in_progress(world);
    let arrival_epoch = crate::vector::arrival_epoch(world);
    let tiles_len = world.tiles.tiles.len();
    let reused = world
        .resources
        .get_mut::<TileRetention>()
        .and_then(|r| r.before_upload.take())
        .filter(|prev| {
            host_frame > 0
                && prev.host_frame == host_frame
                && prev.arrival_epoch == arrival_epoch
                && prev.tiles_len == tiles_len
        });
    let (in_use, drawn) = if let Some(prev) = reused {
        let dropped_gpu = drop_cancelled(world, &prev.in_use, Some(&renderer.queue));
        if dropped_gpu {
            gpu::reclaim_pool_pages(world, Some(&renderer.queue));
        }
        (prev.in_use, prev.drawn)
    } else {
        run_eviction(
            world,
            style,
            view_state,
            Some(&renderer.queue),
            (true, true, renderer.settings.tight_tile_requests),
            &[],
        )
    };
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
    let visible_level = view_state.zoom().zoom_level(DEFAULT_TILE_SIZE);
    ancestors::retain_requested_vector_tiles(
        world,
        style,
        view_state,
        (loose_cache, request_padding, max_zoom, visible_level),
        &mut in_use,
    );
    ancestors::retain_requested_raster_tiles(
        world,
        style,
        view_state,
        (loose_cache, request_padding),
        &mut in_use,
    );
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
    let mut candidates: Vec<(u64, WorldTileCoords, usize)> = {
        let retention = resources.get_or_init_mut::<TileRetention>();
        retention.frame = retention.frame.wrapping_add(1);
        let frame = retention.frame;
        for coords in in_use {
            retention.last_used.insert(*coords, frame);
        }
        retention.bytes_cache.prune_if_oversized(tiles);
        bytes_cache::prune_last_used(&mut retention.last_used, tiles, in_use);
        tiles
            .tiles
            .values()
            .map(|tile| tile.coords)
            .filter(|coords| !in_use.contains(coords) && is_settled(tiles, *coords))
            .map(|coords| {
                (
                    retention.last_used.get(&coords).copied().unwrap_or(0),
                    coords,
                    retention.bytes_cache.get_or_compute(tiles, coords),
                )
            })
            .collect()
    };

    let vector_pool = match resources.get::<Eventually<VectorBufferPool>>() {
        Some(Initialized(pool)) => Some(pool),
        _ => None,
    };
    let symbol_pool = match resources.get::<Eventually<crate::sdf::SymbolBufferPool>>() {
        Some(Initialized(pool)) => Some(pool),
        _ => None,
    };
    if vector_pool.is_some() || symbol_pool.is_some() {
        for (_, coords, bytes) in &mut candidates {
            let gpu_bytes = vector_pool.map_or(0, |p| p.index().tile_gpu_bytes(*coords))
                + symbol_pool.map_or(0, |p| p.index().tile_gpu_bytes(*coords));
            *bytes = bytes.saturating_add(gpu_bytes);
        }
    }
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
            retention.bytes_cache.remove(coords);
        }
    }
    evicted
}

/// Drops the tiles whose requests no one wants any more, preserving tiles that already hold
/// committed geometry as fallback/cache stand-ins while clearing only their pending refresh.
fn drop_cancelled<S: BuildHasher>(
    world: &mut World,
    in_use: &HashSet<WorldTileCoords, S>,
    queue: Option<&crate::render::upload_queue::UploadQueue>,
) -> bool {
    let cancelled = crate::io::tile_retry::cancel_unwanted(world, in_use);
    if cancelled.is_empty() {
        return false;
    }
    tracing::debug!(
        count = cancelled.len(),
        "cancelled requests for tiles out of view"
    );
    let (kept, to_drop): (Vec<_>, Vec<_>) = cancelled
        .into_iter()
        .partition(|&coords| crate::vector::content::has_committed_resident_layers(world, coords));
    for coords in kept {
        crate::vector::content::discard_pending_refresh(world, coords);
    }
    if to_drop.is_empty() {
        ancestors::bump_arrival_epochs(world);
        return false;
    }
    let retention = world.resources.get_or_init_mut::<TileRetention>();
    for coords in &to_drop {
        retention.last_used.remove(coords);
        retention.bytes_cache.remove(coords);
    }
    for coords in &to_drop {
        world.tiles.remove(*coords);
    }
    ancestors::bump_arrival_epochs(world);
    gpu::drop_gpu_data_with_queue(world, &to_drop, queue);
    true
}

#[cfg(test)]
mod tests;
