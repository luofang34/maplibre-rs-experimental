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

use std::collections::{HashMap, HashSet};

use crate::{
    context::MapContext,
    coords::WorldTileCoords,
    io::{
        tile_backpressure::is_settled,
        tile_sources::{clamp_to_max_zoom, source_max_zoom, TileKind},
    },
    raster::resource::RasterResources,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        memory_budget::{MemoryBudget, MemoryPressure},
        projection::view_region_for_projection,
        tile_memory::tile_bytes,
        tile_view_pattern::{WgpuTileViewPattern, DEFAULT_TILE_SIZE},
        view_state::{ViewState, ViewStatePadding},
    },
    style::Style,
    tcs::{
        system::{heap::live_bytes, timings::FrameTimings, SystemResult},
        world::World,
    },
    terrain::{dem_tile_coords, resources::TerrainResources, source::dem_source},
    vector::VectorBufferPool,
};

/// Out-of-view tiles kept per tile in use, as GL JS `MAX_TILE_CACHE_ZOOM_LEVELS`.
const CACHE_TILES_PER_VIEW_TILE: usize = 5;
/// Smallest cache, so a tiny viewport still keeps some history for panning back.
const MIN_CACHE_TILES: usize = 64;
/// Tessellated geometry the out-of-view cache may hold. The tile count is GL JS's, sized for
/// a flat viewport of a few dozen tiles; an eye on terrain draws hundreds, and a planet tile
/// at low zoom weighs tens of megabytes, so the cache is bounded in bytes as well.
const CACHE_BYTES: usize = 128 << 20;
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
    last_used: HashMap<WorldTileCoords, u64>,
    frame: u64,
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
    let drawn = drawn_tiles(world).len();
    let in_use = tiles_in_use(world, style, view_state);
    let evicted = evict_stale_tiles(world, &in_use, drawn);
    if !evicted.is_empty() {
        tracing::debug!(count = evicted.len(), "evicted tiles that left the view");
        drop_gpu_data(world, &evicted);
    }
    if view_state.has_external_view() {
        summarize_residency(world, drawn, in_use.len(), view_state.eye_settled());
        summarize_gpu_objects(world, renderer);
        summarize_timings(world);
    }
    Ok(())
}

/// Once a second, how many GPU objects the frame keeps alive, for a host whose footprint
/// climbs while nothing loads: the kind whose count climbs with it is the one leaking.
fn summarize_gpu_objects(world: &World, renderer: &crate::render::Renderer) {
    let frame = world
        .resources
        .get::<TileRetention>()
        .map_or(0, |retention| retention.frame);
    if !frame.is_multiple_of(SUMMARY_EVERY_FRAMES) {
        return;
    }
    let Some(report) = renderer.instance.generate_report() else {
        return;
    };
    let hub = report.hub_report(renderer.adapter.get_info().backend);
    tracing::info!(
        buffers = hub.buffers.num_allocated,
        textures = hub.textures.num_allocated,
        texture_views = hub.texture_views.num_allocated,
        bind_groups = hub.bind_groups.num_allocated,
        samplers = hub.samplers.num_allocated,
        command_buffers = hub.command_buffers.num_allocated,
        pipelines = hub.render_pipelines.num_allocated,
        "gpu objects"
    );
}

/// Once a second, the costliest systems and stages of the frame, for a host that cannot
/// attach a profiler.
fn summarize_timings(world: &mut World) {
    let frame = world
        .resources
        .get::<TileRetention>()
        .map_or(0, |retention| retention.frame);
    if !frame.is_multiple_of(SUMMARY_EVERY_FRAMES) {
        return;
    }
    let timings = world.resources.get_or_init_mut::<FrameTimings>();
    let growers: Vec<String> = timings
        .top_growth(6)
        .iter()
        .filter(|(_, kb)| *kb > 0.5)
        .map(|(name, kb)| format!("{name}={kb:.1}"))
        .collect();
    let top = timings.take_top(8);
    let costliest: Vec<String> = top
        .iter()
        .map(|(name, ms)| format!("{name}={ms:.2}"))
        .collect();
    tracing::info!(ms_per_frame = %costliest.join(" "), "frame time by system");
    if !growers.is_empty() {
        tracing::info!(
            kb_per_frame = %growers.join(" "),
            rust_heap_mb = live_bytes() >> 20,
            "heap growth by system"
        );
    }
}

fn summarize_residency(world: &World, drawn: usize, in_use: usize, settled: bool) {
    let frame = world
        .resources
        .get::<TileRetention>()
        .map_or(0, |retention| retention.frame);
    if !frame.is_multiple_of(SUMMARY_EVERY_FRAMES) {
        return;
    }
    let tiles = world.tiles.tiles.len();
    let bytes: usize = world
        .tiles
        .tiles
        .values()
        .map(|tile| tile_bytes(&world.tiles, tile.coords))
        .sum();
    let index_bytes = world.tiles.geometry_index.approximate_bytes();
    let pool_revision = match world.resources.get::<Eventually<VectorBufferPool>>() {
        Some(Initialized(pool)) => pool.revision(),
        _ => 0,
    };
    let (drapes, free_drapes, drape_bytes, dem_textures, dem_bytes) =
        match world.resources.get::<Eventually<TerrainResources>>() {
            Some(Initialized(terrain)) => {
                let (held, free) = terrain.drape_counts();
                let (drape_bytes, dem_bytes) = terrain.texture_bytes();
                (
                    held,
                    free,
                    drape_bytes,
                    terrain.dem_texture_count(),
                    dem_bytes,
                )
            }
            _ => (0, 0, 0, 0, 0),
        };
    tracing::info!(
        tiles,
        pending = crate::io::tile_backpressure::tiles_in_flight(&world.tiles),
        settled,
        geometry_mb = bytes >> 20,
        index_mb = index_bytes >> 20,
        pool_revision,
        drawn,
        in_use,
        drapes,
        free_drapes,
        drape_mb = drape_bytes >> 20,
        dem_textures,
        dem_mb = dem_bytes >> 20,
        finest_dem_zoom = finest_dem_zoom(world),
        symbol_revision = symbol_revision(world),
        "resident tiles"
    );
}

/// The tiles the frame draws from, with the tiles whose shapes stand in for them.
pub(crate) fn drawn_tiles(world: &World) -> HashSet<WorldTileCoords> {
    let mut drawn = HashSet::new();
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

/// Every tile the current frame draws from or requests, with the DEM tiles and ancestors
/// terrain reads.
pub(crate) fn tiles_in_use(
    world: &World,
    style: &Style,
    view_state: &ViewState,
) -> HashSet<WorldTileCoords> {
    let mut in_use = drawn_tiles(world);
    match view_region_for_projection(
        style,
        view_state,
        world,
        view_state.zoom().zoom_level(DEFAULT_TILE_SIZE),
        ViewStatePadding::Loose,
    ) {
        Ok(Some(requested)) => {
            // The request systems fetch the ancestor at the source's maximum zoom in place of
            // a finer tile, so that is the tile to keep.
            let max_zoom = source_max_zoom(style, TileKind::Vector);
            for coords in requested.iter() {
                in_use.insert(coords);
                in_use.insert(clamp_to_max_zoom(coords, max_zoom));
            }
        }
        Ok(None) => {}
        Err(error) => tracing::warn!(%error, "cannot select the requested tiles to keep"),
    }
    if let Some(requests) = world
        .resources
        .get::<crate::terrain::request_system::DrapeRequests>()
    {
        in_use.extend(requests.0.iter().copied());
    }
    if let Some(dem) = dem_source(style) {
        let view: Vec<WorldTileCoords> = in_use.iter().copied().collect();
        for coords in view {
            let Some(mut dem_coords) = dem_tile_coords(coords, dem.minzoom, dem.maxzoom) else {
                continue;
            };
            // The coverage index falls back through every ancestor while finer tiles load.
            loop {
                in_use.insert(dem_coords);
                match dem_coords.get_parent() {
                    Some(parent) if u8::from(parent.z) >= dem.minzoom => dem_coords = parent,
                    _ => break,
                }
            }
        }
    }
    in_use
}

/// Removes the least recently used settled tiles beyond the cache budget and returns them;
/// the tile budget follows `view_tiles`, the tiles the frame draws.
pub(crate) fn evict_stale_tiles(
    world: &mut World,
    in_use: &HashSet<WorldTileCoords>,
    view_tiles: usize,
) -> Vec<WorldTileCoords> {
    // A host nearly out of memory keeps nothing beyond what the frame draws from.
    let critical = world
        .resources
        .get::<MemoryBudget>()
        .is_some_and(|budget| budget.pressure() == MemoryPressure::Critical);
    let budget = if critical {
        CacheBudget { tiles: 0, bytes: 0 }
    } else {
        CacheBudget {
            tiles: (view_tiles * CACHE_TILES_PER_VIEW_TILE).max(MIN_CACHE_TILES),
            bytes: CACHE_BYTES,
        }
    };
    evict_beyond(world, in_use, budget)
}

/// Removes the least recently used settled tiles until the rest fit `budget`, in tiles and
/// in bytes of geometry, and returns them.
pub(crate) fn evict_beyond(
    world: &mut World,
    in_use: &HashSet<WorldTileCoords>,
    budget: CacheBudget,
) -> Vec<WorldTileCoords> {
    let World { resources, tiles } = world;
    let retention = resources.get_or_init_mut::<TileRetention>();
    retention.frame = retention.frame.wrapping_add(1);
    let frame = retention.frame;
    for coords in in_use {
        retention.last_used.insert(*coords, frame);
    }

    let mut candidates: Vec<(u64, WorldTileCoords, usize)> = tiles
        .tiles
        .values()
        .map(|tile| tile.coords)
        .filter(|coords| !in_use.contains(coords) && is_settled(tiles, *coords))
        .map(|coords| {
            (
                retention.last_used.get(&coords).copied().unwrap_or(0),
                coords,
                tile_bytes(tiles, coords),
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
    for coords in &evicted {
        tiles.remove(*coords);
        retention.last_used.remove(coords);
    }
    evicted
}

fn drop_gpu_data(world: &mut World, evicted: &[WorldTileCoords]) {
    if let Some(Initialized(pool)) = world.resources.get_mut::<Eventually<VectorBufferPool>>() {
        for coords in evicted {
            pool.remove_tile(*coords);
        }
    }
    if let Some(Initialized(pool)) = world
        .resources
        .get_mut::<Eventually<crate::sdf::SymbolBufferPool>>()
    {
        for coords in evicted {
            pool.remove_tile(*coords);
        }
    }
    if let Some(Initialized(raster)) = world.resources.get_mut::<Eventually<RasterResources>>() {
        for coords in evicted {
            raster.remove_texture(*coords);
        }
    }
    if let Some(Initialized(terrain)) = world.resources.get_mut::<Eventually<TerrainResources>>() {
        for coords in evicted {
            terrain.drop_dem(*coords);
        }
    }
}

#[cfg(test)]
mod tests;

fn finest_dem_zoom(world: &World) -> u8 {
    world
        .tiles
        .tiles
        .values()
        .filter_map(|tile| {
            matches!(
                world
                    .tiles
                    .query::<&crate::terrain::DemTileComponent>(tile.coords),
                Some(crate::terrain::DemTileComponent::Loaded(_))
            )
            .then_some(u8::from(tile.coords.z))
        })
        .max()
        .unwrap_or(0)
}

fn symbol_revision(world: &World) -> u64 {
    match world
        .resources
        .get::<Eventually<crate::sdf::SymbolBufferPool>>()
    {
        Some(Initialized(pool)) => pool.revision(),
        _ => 0,
    }
}
