//! Releases evicted tile buffers, raster/DEM GPU textures, and empty slab pages.

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
    queue: Option<&UploadQueue>,
) {
    if let Some(Initialized(pool)) = world.resources.get_mut::<Eventually<VectorBufferPool>>() {
        for coords in evicted {
            pool.remove_evicted_tile_with_queue(*coords, queue);
        }
    }
    if let Some(Initialized(pool)) = world
        .resources
        .get_mut::<Eventually<crate::sdf::SymbolBufferPool>>()
    {
        for coords in evicted {
            pool.remove_evicted_tile_with_queue(*coords, queue);
        }
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
    if let Some(Initialized(pool)) = world.resources.get_mut::<Eventually<VectorBufferPool>>() {
        pool.reclaim_empty_pages_with_queue(0, queue);
    }
    if let Some(Initialized(pool)) = world
        .resources
        .get_mut::<Eventually<crate::sdf::SymbolBufferPool>>()
    {
        pool.reclaim_empty_pages_with_queue(0, queue);
    }
}
