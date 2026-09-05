//! Evicts tiles that left the view, keeping a recently used cache sized from the viewport.
//!
//! The tile store would otherwise grow for the whole session. GL JS keeps the tiles its
//! covering needs plus a cache of `MAX_TILE_CACHE_ZOOM_LEVELS` times the tiles in view and
//! unloads the rest; this system does the same once per frame in the Cleanup stage. A tile
//! whose worker request is still in flight is never evicted, so its result cannot land on a
//! re-requested tile and duplicate its layers.

use std::collections::{HashMap, HashSet};

use crate::{
    context::MapContext,
    coords::WorldTileCoords,
    raster::{resource::RasterResources, RasterLayersDataComponent},
    render::{
        eventually::{Eventually, Eventually::Initialized},
        tile_view_pattern::WgpuTileViewPattern,
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
pub fn retention_system(MapContext { world, style, .. }: &mut MapContext) -> SystemResult {
    let in_use = tiles_in_use(world, style);
    let evicted = evict_stale_tiles(world, &in_use);
    if !evicted.is_empty() {
        tracing::debug!(count = evicted.len(), "evicted tiles that left the view");
        drop_gpu_data(world, &evicted);
    }
    Ok(())
}

/// Every tile the current frame draws from, with the DEM tiles and ancestors terrain reads.
fn tiles_in_use(world: &World, style: &Style) -> HashSet<WorldTileCoords> {
    let mut in_use = HashSet::new();
    let Some(Initialized(pattern)) = world.resources.get::<Eventually<WgpuTileViewPattern>>()
    else {
        return in_use;
    };
    for view_tile in pattern.iter() {
        in_use.insert(view_tile.coords());
        view_tile.render(|shape| {
            in_use.insert(shape.coords());
        });
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

/// Removes the least recently used settled tiles beyond the cache budget and returns them.
pub(crate) fn evict_stale_tiles(
    world: &mut World,
    in_use: &HashSet<WorldTileCoords>,
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
    let budget = (in_use.len() * CACHE_TILES_PER_VIEW_TILE).max(MIN_CACHE_TILES);
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
