//! Uploads data to the GPU which is needed for rendering.
use std::collections::BTreeSet;

use crate::{
    context::MapContext,
    coords::WorldTileCoords,
    raster::{
        dem_border::{filled_sides, with_border},
        resource::RasterResources,
        AvailableRasterLayerData, RasterLayerData, RasterLayersDataComponent,
    },
    render::{
        eventually::{Eventually, Eventually::Initialized},
        Renderer,
    },
    tcs::{
        system::{SystemError, SystemResult},
        tiles::Tiles,
    },
};

pub fn upload_system(
    MapContext {
        world,
        renderer: Renderer { device, queue, .. },
        ..
    }: &mut MapContext,
) -> SystemResult {
    if crate::render::eye_covering::EyeInFrame::reuses_content(world) {
        return Ok(());
    }
    let Some(Initialized(raster_resources)) = world
        .resources
        .query_mut::<&mut Eventually<RasterResources>>()
    else {
        return Err(SystemError::Dependencies);
    };

    // Every loaded tile gets its texture, as GL JS uploads a raster tile when it arrives; the
    // retention system bounds the set by evicting tiles that left the view.
    let source_tiles: BTreeSet<WorldTileCoords> =
        world.tiles.tiles.values().map(|tile| tile.coords).collect();
    upload_raster_layer(raster_resources, device, queue, &world.tiles, source_tiles);

    Ok(())
}

#[tracing::instrument(skip_all)]
fn upload_raster_layer(
    raster_resources: &mut RasterResources,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    tiles: &Tiles,
    source_tiles: BTreeSet<WorldTileCoords>,
) {
    for coords in source_tiles {
        let Some(layers) = tiles.query::<&RasterLayersDataComponent>(coords) else {
            continue;
        };
        for data in &layers.layers {
            if let RasterLayerData::Available(data) = data {
                if raster_resources
                    .get_bound_texture(&data.source, &coords)
                    .is_none()
                {
                    upload_image(raster_resources, device, queue, tiles, data);
                }
            }
        }
    }
}

/// The image of `source` loaded at each tile around `coords`, indexed `[dy + 1][dx + 1]`.
fn neighbour_images<'a>(
    tiles: &'a Tiles,
    source: &crate::raster::RasterSourceId,
    coords: WorldTileCoords,
) -> [[Option<&'a image::RgbaImage>; 3]; 3] {
    let mut images: [[Option<&image::RgbaImage>; 3]; 3] = Default::default();
    for (neighbour, (dx, dy)) in crate::terrain::backfill::neighbours(coords) {
        images[(dy + 1) as usize][(dx + 1) as usize] = tiles
            .query::<&RasterLayersDataComponent>(neighbour)
            .and_then(|component| {
                component.layers.iter().find_map(|layer| match layer {
                    RasterLayerData::Available(data) if &data.source == source => Some(&data.image),
                    _ => None,
                })
            });
    }
    images
}

fn upload_image(
    raster_resources: &mut RasterResources,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    tiles: &Tiles,
    data: &AvailableRasterLayerData,
) {
    let bordered;
    let mut border_sides = None;
    let image = if raster_resources.has_border(&data.source) {
        let neighbours = neighbour_images(tiles, &data.source, data.coords);
        border_sides = Some(filled_sides(&data.image, &neighbours));
        bordered = with_border(&data.image, &neighbours);
        &bordered
    } else {
        &data.image
    };
    let (width, height) = image.dimensions();

    let texture = raster_resources.create_texture(
        None,
        device,
        // Raster style colors are sampled in the encoded color space, matching WebGL's
        // default RGBA upload path. An sRGB texture view would decode the texels to linear
        // values before writing them to the non-sRGB render target, making imagery too dark.
        wgpu::TextureFormat::Rgba8Unorm,
        width,
        height,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
    );

    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            aspect: wgpu::TextureAspect::All,
            texture: &texture.texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
        },
        image,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * width),
            rows_per_image: Some(height),
        },
        texture.size,
    );

    raster_resources.bind_texture(device, &data.source, &data.coords, texture);
    if let Some(sides) = border_sides {
        raster_resources.set_border_sides(&data.source, data.coords, sides);
    }
}
