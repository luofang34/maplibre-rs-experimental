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
    schedule::system::{SystemError, SystemResult},
    tcs::tiles::Tiles,
    terrain::{DemLayersComponent, GpuDemPool},
};

#[derive(Default)]
struct RasterUploadCache {
    last_key: Option<(u64, u64, u64, u64, usize)>,
    last_deferred: bool,
}

pub fn upload_system(
    MapContext {
        style,
        view_state,
        world,
        renderer: Renderer { device, queue, .. },
        ..
    }: &mut MapContext,
) -> SystemResult {
    let Some(Initialized(raster_resources)) = world.resources.get::<Eventually<RasterResources>>()
    else {
        return Ok(());
    };
    let pre_raster_gen = raster_resources.mutation_generation();
    if crate::render::eye_covering::EyeInFrame::reuses_content(world) {
        return Ok(());
    }
    let has_media_sources = style.cached_has_media_sources();
    let has_raster_or_media = style
        .cached_has_any_raster_type(&crate::io::tile_sources::RASTER_LAYER_TYPES)
        || has_media_sources;
    if !has_raster_or_media
        && world
            .tiles
            .pyramid_of::<RasterLayersDataComponent>()
            .is_empty()
        && world.tiles.pyramid_of::<DemLayersComponent>().is_empty()
    {
        return Ok(());
    }
    let has_active_media = has_media_sources
        && style.sources.values().any(|s| {
            matches!(
                s,
                crate::style::source::Source::Video(v) if v.frame.is_some()
            ) || matches!(
                s,
                crate::style::source::Source::Canvas(c) if c.frame.is_some()
            )
        });
    let pre_key = (
        crate::raster::raster_arrival_epoch(world),
        crate::terrain::dem_arrival_epoch(world),
        pre_raster_gen,
        style.revision(),
        world.tiles.len(),
    );
    if !has_active_media
        && world
            .resources
            .get::<RasterUploadCache>()
            .is_some_and(|c| !c.last_deferred && c.last_key == Some(pre_key))
    {
        return Ok(());
    }
    if has_active_media {
        crate::raster::media_source::sync_media_sources(style, view_state, world);
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
    let source_tiles: Vec<WorldTileCoords> = world
        .tiles
        .pyramid_of::<RasterLayersDataComponent>()
        .coords_coarse_to_fine()
        .collect();
    let mut deferred = upload_raster_layer(
        raster_resources,
        (device, queue),
        &mut world.tiles,
        source_tiles,
        budget,
    );
    if !deferred {
        let dem_tiles = world
            .tiles
            .pyramid_of::<DemLayersComponent>()
            .coords_coarse_to_fine();
        deferred |= upload_dem_layers(
            style,
            raster_resources,
            gpu_dem_pool,
            (device, queue),
            &world.tiles,
            dem_tiles,
            budget,
        );
    }
    let post_raster_gen = raster_resources.mutation_generation();
    let post_key = (
        crate::raster::raster_arrival_epoch(world),
        crate::terrain::dem_arrival_epoch(world),
        post_raster_gen,
        style.revision(),
        world.tiles.len(),
    );
    world.resources.insert(RasterUploadCache {
        last_key: Some(post_key),
        last_deferred: deferred,
    });
    if deferred {
        crate::render::frame_signals::keep_animating(world);
    }

    Ok(())
}

/// Uploads the imagery textures of loaded tiles that the frame's budget admits, immediately
/// releasing uploaded CPU `RgbaImage` pixel buffers; returns `true` when some wait for a later frame.
#[tracing::instrument(skip_all)]
fn upload_raster_layer(
    raster_resources: &mut RasterResources,
    (device, queue): (&wgpu::Device, &crate::render::upload_queue::UploadQueue),
    tiles: &mut Tiles,
    source_tiles: impl IntoIterator<Item = WorldTileCoords>,
    budget: &mut FrameUploads,
) -> bool {
    let mut mipmapped = Vec::new();
    let mut deferred = false;
    'tiles: for coords in source_tiles {
        let Some(layers) = tiles.query_mut::<&mut RasterLayersDataComponent>(coords) else {
            continue;
        };
        for data in &mut layers.layers {
            if let RasterLayerData::Available(data) = data {
                if data.image.is_empty() {
                    continue;
                }
                // A result that lands after its source left the style is never drawn.
                if !raster_resources.draws(&data.source) {
                    continue;
                }
                if raster_resources
                    .get_bound_texture(&data.source, &coords)
                    .is_some()
                {
                    data.image = image::RgbaImage::new(0, 0);
                    continue;
                }
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
                data.image = image::RgbaImage::new(0, 0);
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
    style: &crate::style::Style,
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
            let pool_source = crate::terrain::dem_store::canonical_dem_source(style, source);
            let needs_gpu_write = gpu_dem_pool.needs_upload(&pool_source, coords, dem);
            if needs_gpu_write && !budget.take(UploadKind::Image, dem.pixels().len()) {
                deferred = true;
                break 'tiles;
            }
            let entry = if needs_gpu_write {
                gpu_dem_pool.upload(device, queue, &pool_source, coords, dem)
            } else if let Some(existing) = gpu_dem_pool.get(&pool_source, coords) {
                existing
            } else {
                gpu_dem_pool.upload(device, queue, &pool_source, coords, dem)
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
            texture: texture.as_ref(),
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
        },
        &data.image,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * width),
            rows_per_image: Some(height),
        },
        texture.extent(),
    );

    let mips = texture.as_ref().clone();
    raster_resources.bind_texture(device, &data.source, &coords, texture);
    mips
}

#[cfg(test)]
mod tests;
