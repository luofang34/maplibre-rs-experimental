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
    io::tile_sources::{clamp_to_max_zoom, source_max_zoom, TileKind},
    raster::{resource::RasterResources, RasterLayersDataComponent},
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::view_region_for_projection,
        tile_view_pattern::{WgpuTileViewPattern, DEFAULT_TILE_SIZE},
        view_state::{ViewState, ViewStatePadding},
    },
    style::Style,
    tcs::{system::SystemResult, tiles::Tiles, world::World},
    terrain::{dem_tile_coords, resources::TerrainResources, source::dem_source, DemTileComponent},
    vector::VectorLayerBucketComponent,
};

/// Out-of-view tiles kept per tile in use, as GL JS `MAX_TILE_CACHE_ZOOM_LEVELS`.
const CACHE_TILES_PER_VIEW_TILE: usize = 5;
/// Smallest cache, so a tiny viewport still keeps some history for panning back.
const MIN_CACHE_TILES: usize = 64;

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
        ..
    }: &mut MapContext,
) -> SystemResult {
    let drawn = drawn_tiles(world).len();
    let in_use = tiles_in_use(world, style, view_state);
    let evicted = evict_stale_tiles(world, &in_use, drawn);
    if !evicted.is_empty() {
        tracing::debug!(count = evicted.len(), "evicted tiles that left the view");
        drop_gpu_data(world, &evicted);
    }
    Ok(())
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
/// the budget follows `view_tiles`, the tiles the frame draws.
pub(crate) fn evict_stale_tiles(
    world: &mut World,
    in_use: &HashSet<WorldTileCoords>,
    view_tiles: usize,
) -> Vec<WorldTileCoords> {
    let World { resources, tiles } = world;
    let retention = resources.get_or_init_mut::<TileRetention>();
    retention.frame = retention.frame.wrapping_add(1);
    let frame = retention.frame;
    for coords in in_use {
        retention.last_used.insert(*coords, frame);
    }

    let mut candidates: Vec<(u64, WorldTileCoords)> = tiles
        .tiles
        .values()
        .map(|tile| tile.coords)
        .filter(|coords| !in_use.contains(coords) && is_settled(tiles, *coords))
        .map(|coords| {
            (
                retention.last_used.get(&coords).copied().unwrap_or(0),
                coords,
            )
        })
        .collect();
    let budget = (view_tiles * CACHE_TILES_PER_VIEW_TILE).max(MIN_CACHE_TILES);
    if candidates.len() <= budget {
        return Vec::new();
    }
    candidates.sort_by_key(|(used, _)| *used);
    let evicted: Vec<WorldTileCoords> = candidates[..candidates.len() - budget]
        .iter()
        .map(|(_, coords)| *coords)
        .collect();
    for coords in &evicted {
        tiles.remove(*coords);
        retention.last_used.remove(coords);
    }
    evicted
}

/// Whether every request for the tile has produced a result.
fn is_settled(tiles: &Tiles, coords: WorldTileCoords) -> bool {
    let vector_loading = tiles
        .query::<&VectorLayerBucketComponent>(coords)
        .is_some_and(|component| !component.done);
    let raster_loading = tiles
        .query::<&RasterLayersDataComponent>(coords)
        .is_some_and(|component| component.layers.is_empty());
    let dem_loading = matches!(
        tiles.query::<&DemTileComponent>(coords),
        Some(DemTileComponent::Pending)
    );
    !(vector_loading || raster_loading || dem_loading)
}

fn drop_gpu_data(world: &mut World, evicted: &[WorldTileCoords]) {
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
