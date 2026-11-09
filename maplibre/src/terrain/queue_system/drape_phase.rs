//! Encodes the source layers and masks of terrain drape textures.
use crate::{
    coords::{Zoom, TILE_SIZE},
    hillshade::render_commands::DrawDemTiles,
    projection::renderer_data::tile_mercator_coordinates,
    raster::render_commands::DrawRasterTiles,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        render_commands::DrawMasks,
        render_phase::{Draw, DrawState, LayerItem, ProjectionBinding, TileMaskItem},
        shaders::ShaderTileMetadata,
        tile_view_pattern::{TileShape, WgpuTileViewPattern},
    },
    style::{source::TileAddressingScheme, Style},
    tcs::{system::SystemError, tiles::Tile, world::World},
    terrain::{
        drape_targets::TargetSpec,
        resources::TerrainResources,
        rtt::{drape_transform, EDGE_OVERLAP},
        DrapePhase, DrapeTarget,
    },
    vector::render_commands::{DrawLineTiles, DrawVectorTiles},
};
type TargetSlots = Vec<Option<usize>>;
/// Instance metadata placing each redrawn target's source shapes inside its drape texture.
///
/// Shapes past `capacity` get no slot: a view falling back to many small child tiles can ask
/// for more than the metadata buffer holds, and those shapes wait for their own tiles.
pub(super) fn drape_metadata(
    specs: &[TargetSpec],
    redraw: &[bool],
    capacity: usize,
    view_zoom: Zoom,
    drape_size: u32,
) -> (Vec<ShaderTileMetadata>, Vec<TargetSlots>) {
    let mut metadata = Vec::new();
    let mut slots = Vec::with_capacity(specs.len());
    let mut skipped = 0_usize;
    for (spec, redraw) in specs.iter().zip(redraw) {
        let texture_zoom = Zoom::new(
            f64::from(u8::from(spec.coords.z)) + (f64::from(drape_size) / TILE_SIZE).log2(),
        );
        let mut target_slots = Vec::with_capacity(spec.shapes.len());
        for shape in &spec.shapes {
            let transform = redraw
                .then(|| drape_transform(spec.coords, shape.source))
                .flatten()
                .and_then(|transform| transform.cast::<f32>());
            let Some(transform) = transform else {
                target_slots.push(None);
                continue;
            };
            if metadata.len() >= capacity {
                target_slots.push(None);
                skipped += 1;
                continue;
            }
            target_slots.push(Some(metadata.len()));
            metadata.push(ShaderTileMetadata {
                transform: transform.into(),
                zoom_factor: texture_zoom.scale_to_tile(&shape.source) as f32,
                viewport_width: drape_size as f32,
                viewport_height: drape_size as f32,
                tile_mercator_coords: tile_mercator_coordinates(
                    shape.source.into_tile(TileAddressingScheme::XYZ),
                )
                .into(),
                clip_antimeridian: 0,
                line_units_per_pixel: 8.0 * texture_zoom.scale_to_tile(&shape.source) as f32,
                // A raster tile reaches a hair past its edges; a vector tile's lines scale with the
                // texture's zoom.
                line_width_scale: if shape.raster_layers.is_empty() {
                    2.0_f64.powf(texture_zoom.value() - view_zoom.value()) as f32
                } else {
                    (1.0 + EDGE_OVERLAP) as f32
                },
                clip_region: crate::render::shaders::WHOLE_TILE,
            });
        }
        slots.push(target_slots);
    }
    if skipped > 0 {
        tracing::warn!(
            skipped,
            capacity,
            "drape shapes exceed the metadata buffer; some tiles drape without them this frame"
        );
    }
    (metadata, slots)
}

pub(super) fn build_drape_phase(
    specs: &[TargetSpec],
    redraw: &[bool],
    slots: &[TargetSlots],
    ranges: &[std::ops::Range<wgpu::BufferAddress>],
    zoom: Zoom,
    clear_color: wgpu::Color,
) -> DrapePhase {
    let mut phase = DrapePhase::default();
    for ((spec, redraw), target_slots) in specs.iter().zip(redraw).zip(slots) {
        if !redraw {
            continue;
        }
        let mut target = DrapeTarget {
            coords: spec.coords,
            clear_color,
            masks: Vec::new(),
            layers: Vec::new(),
        };
        for (shape, slot) in spec.shapes.iter().zip(target_slots) {
            let Some(range) = slot.and_then(|index| ranges.get(index)) else {
                continue;
            };
            let source_shape = TileShape::with_buffer_range(shape.source, zoom, range.clone());
            target.masks.push(TileMaskItem {
                draw_function: Box::new(DrawState::<TileMaskItem, DrawMasks>::new()),
                source_shape: source_shape.clone(),
                generate_borders: false,
                projection: ProjectionBinding::Flat,
            });
            for layer in &shape.vector_layers {
                let draw_function: Box<dyn Draw<LayerItem>> = if layer.is_line {
                    Box::new(DrawState::<LayerItem, DrawLineTiles>::new())
                } else {
                    Box::new(DrawState::<LayerItem, DrawVectorTiles>::new())
                };
                target.layers.push(LayerItem {
                    draw_function,
                    index: layer.index,
                    generate_borders: false,
                    style_layer: layer.id.clone(),
                    tile: Tile {
                        coords: layer.coords,
                    },
                    source_shape: source_shape.clone(),
                    projection: ProjectionBinding::Flat,
                    run: None,
                });
            }
            for (id, index, dem) in &shape.raster_layers {
                let draw_function: Box<dyn Draw<LayerItem>> = if *dem {
                    Box::new(DrawState::<LayerItem, DrawDemTiles>::new())
                } else {
                    Box::new(DrawState::<LayerItem, DrawRasterTiles>::new())
                };
                target.layers.push(LayerItem {
                    draw_function,
                    index: *index,
                    generate_borders: false,
                    style_layer: id.clone(),
                    tile: Tile {
                        coords: shape.source,
                    },
                    source_shape: source_shape.clone(),
                    projection: ProjectionBinding::Flat,
                    run: None,
                });
            }
        }
        target.layers.sort_by_key(|item| item.index);
        phase.targets.push(target);
    }
    phase
}

/// Color the drape textures start from: the constant background paint, a neutral cartographic
/// terrain tone when terrain is active without a background layer, or transparent.
pub(super) fn background_clear_color(style: &Style) -> wgpu::Color {
    style
        .layers
        .iter()
        .find(|layer| layer.type_ == "background" && !layer.is_hidden())
        .and_then(|layer| layer.paint.as_ref()?.get_color())
        .map(|color| wgpu::Color {
            r: f64::from(color.color.r),
            g: f64::from(color.color.g),
            b: f64::from(color.color.b),
            a: f64::from(color.alpha),
        })
        .unwrap_or_else(|| {
            if style.terrain.is_some() {
                wgpu::Color {
                    r: 0.91,
                    g: 0.89,
                    b: 0.85,
                    a: 1.0,
                }
            } else {
                wgpu::Color::TRANSPARENT
            }
        })
}

pub(super) fn encode_drapes(
    specs: &[TargetSpec],
    redraw: &[bool],
    clear_color: wgpu::Color,
    world: &mut World,
    zoom: Zoom,
    queue: &crate::render::upload_queue::UploadQueue,
) -> Result<DrapePhase, SystemError> {
    let capacity = match world.resources.get::<Eventually<WgpuTileViewPattern>>() {
        Some(Initialized(pattern)) => pattern.remaining_metadata_capacity(),
        _ => return Err(SystemError::Dependencies),
    };

    let drape_size = match world.resources.get::<Eventually<TerrainResources>>() {
        Some(Initialized(terrain)) => terrain.drape_size(),
        _ => return Err(SystemError::Dependencies),
    };
    let (metadata, slots) = drape_metadata(specs, redraw, capacity, zoom, drape_size);
    let ranges = {
        let Some(Initialized(pattern)) =
            world.resources.get_mut::<Eventually<WgpuTileViewPattern>>()
        else {
            return Err(SystemError::Dependencies);
        };
        pattern
            .upload_extra_metadata(queue, &metadata)
            .map_err(|error| {
                tracing::error!(%error, "unable to upload drape metadata");
                SystemError::Setup
            })?
    };
    Ok(build_drape_phase(
        specs,
        redraw,
        &slots,
        &ranges,
        zoom,
        clear_color,
    ))
}

#[cfg(test)]
mod tests;
