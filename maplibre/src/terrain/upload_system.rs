//! Uploads decoded DEM tiles to the shared GPU elevation pool and binds them for terrain meshes.

use crate::{
    context::MapContext,
    io::tile_sources::RasterSourceId,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        memory_budget::{FrameUploads, UploadKind},
        Renderer,
    },
    tcs::system::SystemResult,
    terrain::{
        gpu_dem_pool::GpuDemPool, resources::TerrainResources, source::dem_source, DemTileComponent,
    },
};

#[derive(Default)]
struct TerrainUploadCache {
    last_key: Option<(u64, u64, usize, u64, usize, usize)>,
    last_deferred: bool,
}

pub fn upload_system(
    MapContext {
        style,
        world,
        renderer: Renderer { device, queue, .. },
        ..
    }: &mut MapContext,
) -> SystemResult {
    if crate::render::eye_covering::EyeInFrame::reuses_content(world) {
        return Ok(());
    }
    let Some(dem_spec) = dem_source(style) else {
        return Ok(());
    };
    let source_id = crate::terrain::dem_store::canonical_dem_source(
        style,
        &RasterSourceId::from(dem_spec.name.as_str()),
    );
    world.resources.get_or_init_mut::<FrameUploads>();
    world.resources.get_or_init_mut::<GpuDemPool>();
    let Some(Initialized(terrain_resources)) =
        world.resources.get::<Eventually<TerrainResources>>()
    else {
        return Ok(());
    };
    let dem_arrival_epoch = crate::terrain::dem_arrival_epoch(world);
    let wanted_epoch = crate::terrain::request_system::wanted_dems_epoch(world);
    let wanted_dems = world
        .resources
        .get::<crate::terrain::request_system::TerrainWantedDems>();
    let wanted_len = wanted_dems.map_or(usize::MAX, |w| w.0.len());
    let key = (
        dem_arrival_epoch,
        wanted_epoch,
        wanted_len,
        style.revision(),
        world.tiles.tiles.len(),
        terrain_resources.dem_texture_count(),
    );
    if world
        .resources
        .get::<TerrainUploadCache>()
        .is_some_and(|c| !c.last_deferred && c.last_key == Some(key))
    {
        return Ok(());
    }
    let to_upload: Vec<_> = world
        .tiles
        .pyramid_of::<DemTileComponent>()
        .coords_coarse_to_fine()
        .filter_map(|coords| {
            if wanted_dems.is_some_and(|wanted| !wanted.0.contains(&coords)) {
                return None;
            }
            let Some(DemTileComponent::Loaded(dem)) =
                world.tiles.query::<&DemTileComponent>(coords)
            else {
                return None;
            };
            terrain_resources
                .needs_dem_upload(coords, dem)
                .then(|| (coords, dem.clone()))
        })
        .collect();
    if to_upload.is_empty() {
        world.resources.insert(TerrainUploadCache {
            last_key: Some(key),
            last_deferred: false,
        });
        return Ok(());
    }

    let mut deferred = false;
    for (coords, dem) in to_upload {
        let needs_gpu_write = world
            .resources
            .get::<GpuDemPool>()
            .is_none_or(|pool| pool.needs_upload(&source_id, coords, &dem));
        if needs_gpu_write {
            let Some(budget) = world.resources.get_mut::<FrameUploads>() else {
                break;
            };
            if !budget.take(UploadKind::Image, dem.tile.pixels().len()) {
                deferred = true;
                break;
            }
        }
        let shared = {
            let pool = world.resources.get_or_init_mut::<GpuDemPool>();
            let entry = if needs_gpu_write {
                pool.upload(device, queue, &source_id, coords, &dem)
            } else if let Some(existing) = pool.get(&source_id, coords) {
                existing
            } else {
                pool.upload(device, queue, &source_id, coords, &dem)
            };
            entry
                .texture
                .clone_handle()
                .map(|texture| (texture, entry.revision))
        };
        if let Some((Initialized(terrain_resources), (texture, revision))) = world
            .resources
            .get_mut::<Eventually<TerrainResources>>()
            .zip(shared)
        {
            terrain_resources.bind_shared_dem(coords, texture, revision, dem.revision_key());
        } else if let Some(Initialized(terrain_resources)) =
            world.resources.get_mut::<Eventually<TerrainResources>>()
        {
            terrain_resources.upload_loaded_dem(device, queue, coords, &dem);
        }
    }
    let next_dem_count = world
        .resources
        .get::<Eventually<TerrainResources>>()
        .and_then(|res| match res {
            Initialized(terrain_resources) => Some(terrain_resources.dem_texture_count()),
            _ => None,
        })
        .unwrap_or(0);
    world.resources.insert(TerrainUploadCache {
        last_key: Some((key.0, key.1, key.2, key.3, key.4, next_dem_count)),
        last_deferred: deferred,
    });
    if deferred {
        crate::render::frame_signals::keep_animating(world);
    }
    Ok(())
}
