//! Encodes the source layers and masks of terrain drape textures.
use crate::{
    coords::{Zoom, TILE_SIZE},
    hillshade::render_commands::DrawDemTiles,
    projection::renderer_data::tile_mercator_coordinates,
    raster::render_commands::DrawRasterTiles,
    render::{
        render_commands::DrawMasks,
        render_phase::{Draw, DrawState, LayerItem, ProjectionBinding, TileMaskItem},
        shaders::ShaderTileMetadata,
        tile_view_pattern::TileShape,
    },
    style::{source::TileAddressingScheme, Style},
    tcs::tiles::Tile,
    terrain::{
        drape_targets::TargetSpec, resources::DRAPE_SIZE, rtt::drape_transform, DrapePhase,
        DrapeTarget,
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
) -> (Vec<ShaderTileMetadata>, Vec<TargetSlots>) {
    let mut metadata = Vec::new();
    let mut slots = Vec::with_capacity(specs.len());
    let mut skipped = 0_usize;
    for (spec, redraw) in specs.iter().zip(redraw) {
        let texture_zoom = Zoom::new(
            f64::from(u8::from(spec.coords.z)) + (f64::from(DRAPE_SIZE) / TILE_SIZE).log2(),
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
                viewport_width: DRAPE_SIZE as f32,
                viewport_height: DRAPE_SIZE as f32,
                tile_mercator_coords: tile_mercator_coordinates(
                    shape.source.into_tile(TileAddressingScheme::XYZ),
                )
                .into(),
                clip_antimeridian: 0,
                line_units_per_pixel: 8.0 * texture_zoom.scale_to_tile(&shape.source) as f32,
                line_width_scale: 2.0_f64.powf(texture_zoom.value() - view_zoom.value()) as f32,
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
                    is_line: layer.is_line,
                    generate_borders: false,
                    style_layer: layer.id.clone(),
                    tile: Tile {
                        coords: layer.coords,
                    },
                    source_shape: source_shape.clone(),
                    projection: ProjectionBinding::Flat,
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
                    is_line: false,
                    generate_borders: false,
                    style_layer: id.clone(),
                    tile: Tile {
                        coords: shape.source,
                    },
                    source_shape: source_shape.clone(),
                    projection: ProjectionBinding::Flat,
                });
            }
        }
        target.layers.sort_by_key(|item| item.index);
        phase.targets.push(target);
    }
    phase
}

/// Color the drape textures start from: the constant background paint, or transparent.
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
        .unwrap_or(wgpu::Color::TRANSPARENT)
}

#[cfg(test)]
mod tests;
