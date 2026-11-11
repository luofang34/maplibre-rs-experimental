//! Uploads raster imagery and shared `raster-dem` elevation textures to the GPU.

use crate::{
    context::MapContext,
    coords::WorldTileCoords,
    raster::{
        resource::RasterResources, AvailableRasterLayerData, RasterLayerData,
        RasterLayersDataComponent,
    },
    render::{
        eventually::{Eventually, Eventually::Initialized},
        memory_budget::{FrameUploads, UploadKind},
        Renderer,
    },
    tcs::{
        system::{SystemError, SystemResult},
        tiles::Tiles,
    },
    terrain::{DemLayersComponent, GpuDemPool},
};

pub fn upload_system(
    MapContext {
        world,
        renderer: Renderer { device, queue, .. },
        ..
    }: &mut MapContext,
) -> SystemResult {
    let Some(raster_state) = world.resources.get::<Eventually<RasterResources>>() else {
        return Ok(());
    };
    if !matches!(raster_state, Initialized(_)) {
        return Err(SystemError::Dependencies);
    }
    if crate::render::eye_covering::EyeInFrame::reuses_content(world) {
        return Ok(());
    }
    world.resources.get_or_init_mut::<FrameUploads>();
    world.resources.get_or_init_mut::<GpuDemPool>();
    let Some((Initialized(raster_resources), budget, gpu_dem_pool)) = world.resources.query_mut::<(
        &mut Eventually<RasterResources>,
        &mut FrameUploads,
        &mut GpuDemPool,
    )>() else {
        return Err(SystemError::Dependencies);
    };

    // Every loaded tile gets its texture, as GL JS uploads a raster tile when it arrives, as
    // far as the frame's image floor reaches; coarser tiles go first so an ancestor stand-in is
    // ready before a burst of finer children exhausts the frame budget.
    let source_tiles = world
        .tiles
        .pyramid_of::<RasterLayersDataComponent>()
        .coords_coarse_to_fine();
    let mut deferred = upload_raster_layer(
        raster_resources,
        (device, queue),
        &world.tiles,
        source_tiles,
        budget,
    );
    if !deferred {
        let dem_tiles = world
            .tiles
            .pyramid_of::<DemLayersComponent>()
            .coords_coarse_to_fine();
        deferred |= upload_dem_layers(
            raster_resources,
            gpu_dem_pool,
            (device, queue),
            &world.tiles,
            dem_tiles,
            budget,
        );
    }
    if deferred {
        crate::render::frame_signals::keep_animating(world);
    }

    Ok(())
}

/// Uploads the imagery textures of loaded tiles that the frame's budget admits; `true` when some
/// wait for a later frame.
#[tracing::instrument(skip_all)]
fn upload_raster_layer(
    raster_resources: &mut RasterResources,
    (device, queue): (&wgpu::Device, &crate::render::upload_queue::UploadQueue),
    tiles: &Tiles,
    source_tiles: impl IntoIterator<Item = WorldTileCoords>,
    budget: &mut FrameUploads,
) -> bool {
    let mut mipmapped = Vec::new();
    let mut deferred = false;
    'tiles: for coords in source_tiles {
        let Some(layers) = tiles.query::<&RasterLayersDataComponent>(coords) else {
            continue;
        };
        for data in &layers.layers {
            if let RasterLayerData::Available(data) = data {
                if raster_resources.has_border(&data.source) && data.image.is_empty() {
                    continue;
                }
                // A result that lands after its source left the style is never drawn.
                if raster_resources.draws(&data.source)
                    && raster_resources
                        .get_bound_texture(&data.source, &coords)
                        .is_none()
                {
                    if !budget.take(UploadKind::Image, data.image.as_raw().len()) {
                        deferred = true;
                        break 'tiles;
                    }
                    mipmapped.push(upload_imagery(
                        raster_resources,
                        device,
                        queue,
                        coords,
                        data,
                    ));
                }
            }
        }
    }
    // One submission fills every new texture's mip chain.
    raster_resources.generate_mipmaps(device, queue, &mipmapped);
    deferred
}

/// Uploads or reuses shared `(dim + 4) x (dim + 4)` DEM textures in [`GpuDemPool`] for every
/// `hillshade` and `color-relief` source drawn by the style.
#[tracing::instrument(skip_all)]
fn upload_dem_layers(
    raster_resources: &mut RasterResources,
    gpu_dem_pool: &mut GpuDemPool,
    (device, queue): (&wgpu::Device, &crate::render::upload_queue::UploadQueue),
    tiles: &Tiles,
    dem_tiles: impl IntoIterator<Item = WorldTileCoords>,
    budget: &mut FrameUploads,
) -> bool {
    let mut deferred = false;
    'tiles: for coords in dem_tiles {
        let Some(layers) = tiles.query::<&DemLayersComponent>(coords) else {
            continue;
        };
        for (source, dem) in layers.iter() {
            if !raster_resources.draws(source)
                || raster_resources.dem_is_current(source, coords, dem)
            {
                continue;
            }
            let needs_gpu_write = gpu_dem_pool.needs_upload(source, coords, dem);
            if needs_gpu_write && !budget.take(UploadKind::Image, dem.tile.pixels().len()) {
                deferred = true;
                break 'tiles;
            }
            let entry = if needs_gpu_write {
                gpu_dem_pool.upload(device, queue, source, coords, dem)
            } else if let Some(existing) = gpu_dem_pool.get(source, coords) {
                existing
            } else {
                gpu_dem_pool.upload(device, queue, source, coords, dem)
            };
            raster_resources.bind_dem_texture(
                device,
                source,
                coords,
                &entry.texture,
                dem.revision_key(),
            );
        }
    }
    deferred
}

/// Uploads an imagery tile stored at `coords` and returns its `wgpu::Texture` handle for mipmap
/// generation.
fn upload_imagery(
    raster_resources: &mut RasterResources,
    device: &wgpu::Device,
    queue: &crate::render::upload_queue::UploadQueue,
    coords: WorldTileCoords,
    data: &AvailableRasterLayerData,
) -> wgpu::Texture {
    let (width, height) = data.image.dimensions();

    // Raster style colors are sampled in the encoded color space, matching WebGL's default
    // RGBA upload path. An sRGB texture view would decode the texels to linear values before
    // writing them to the non-sRGB render target, making imagery too dark.
    let texture = raster_resources.create_imagery_texture(device, width, height);

    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            aspect: wgpu::TextureAspect::All,
            texture: &texture.texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
        },
        &data.image,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * width),
            rows_per_image: Some(height),
        },
        texture.size,
    );

    let mips = texture.texture.clone();
    raster_resources.bind_texture(device, &data.source, &coords, texture);
    mips
}

#[cfg(test)]
mod tests;
