//! Releases evicted tile buffers, raster/DEM GPU textures, and empty slab pages.

use rustc_hash::FxHashSet;

#[cfg(feature = "raster")]
use crate::raster::resource::RasterResources;
use crate::{
    coords::WorldTileCoords,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        upload_queue::UploadQueue,
    },
    tcs::world::World,
    terrain::resources::TerrainResources,
    vector::VectorBufferPool,
};

pub(super) fn drop_gpu_data_with_queue(
    world: &mut World,
    evicted: &[WorldTileCoords],
    _queue: Option<&UploadQueue>,
) {
    if evicted.is_empty() {
        return;
    }
    let evicted_set: FxHashSet<WorldTileCoords> = evicted.iter().copied().collect();
    if let Some(Initialized(pool)) = world.resources.get_mut::<Eventually<VectorBufferPool>>() {
        pool.remove_evicted_tiles_batch(&evicted_set);
    }
    if let Some(Initialized(pool)) = world
        .resources
        .get_mut::<Eventually<crate::sdf::SymbolBufferPool>>()
    {
        pool.remove_evicted_tiles_batch(&evicted_set);
    }
    #[cfg(feature = "raster")]
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
    if let Some(pool) = world.resources.get_mut::<crate::terrain::GpuDemPool>() {
        for coords in evicted {
            pool.drop_coords(*coords);
        }
    }
}

pub(super) fn reclaim_pool_pages(world: &mut World, queue: Option<&UploadQueue>) {
    let is_tight = world
        .resources
        .get::<crate::render::memory_budget::MemoryBudget>()
        .is_some_and(crate::render::memory_budget::MemoryBudget::is_tight);
    let keep_spare = usize::from(!is_tight);
    if let Some(Initialized(pool)) = world.resources.get_mut::<Eventually<VectorBufferPool>>() {
        pool.reclaim_empty_pages_with_queue(keep_spare, queue);
    }
    if let Some(Initialized(pool)) = world
        .resources
        .get_mut::<Eventually<crate::sdf::SymbolBufferPool>>()
    {
        pool.reclaim_empty_pages_with_queue(keep_spare, queue);
    }
}
