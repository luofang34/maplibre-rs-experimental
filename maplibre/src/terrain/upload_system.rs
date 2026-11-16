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
    let max_terrain_dem_z = world
        .resources
        .get::<crate::terrain::TerrainCoverageIndex>()
        .and_then(|idx| idx.max_rendered_zoom())
        .map(|surface_z| {
            let drape_max = world
                .resources
                .get::<crate::terrain::request_system::DrapeRequests>()
                .into_iter()
                .flat_map(|r| r.0.iter())
                .chain(
                    world
                        .resources
                        .get::<crate::terrain::request_system::DrapePrefetchRequests>()
                        .into_iter()
                        .flat_map(|r| r.0.iter()),
                )
                .map(|c| u8::from(c.z))
                .max()
                .unwrap_or(surface_z);
            surface_z
                .max(drape_max)
                .saturating_sub(1)
                .min(dem_spec.maxzoom)
        });
    let Some(Initialized(terrain_resources)) =
        world.resources.get::<Eventually<TerrainResources>>()
    else {
        return Ok(());
    };
    let to_upload: Vec<_> = world
        .tiles
        .pyramid_of::<DemTileComponent>()
        .coords_coarse_to_fine()
        .filter_map(|coords| {
            if max_terrain_dem_z.is_some_and(|max_z| u8::from(coords.z) > max_z) {
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
    if deferred {
        crate::render::frame_signals::keep_animating(world);
    }
    Ok(())
}
