//! Queues [PhaseItems](crate::render::render_phase::PhaseItem) for rendering.

use crate::{
    context::MapContext,
    raster::{paint::RasterUniforms, render_commands::DrawRasterTiles, resource::RasterResources},
    render::{
        eventually::{Eventually, Eventually::Initialized},
        render_commands::DrawMasks,
        render_phase::{DrawState, LayerItem, ProjectionBinding, RenderPhase, TileMaskItem},
        tile_view_pattern::WgpuTileViewPattern,
        Renderer,
    },
    style::layer::{LayerPaint, RasterPaint},
    tcs::{
        system::{SystemError, SystemResult},
        tiles::Tile,
    },
};

pub fn queue_system(
    MapContext {
        style,
        view_state,
        world,
        renderer: Renderer { device, queue, .. },
        ..
    }: &mut MapContext,
) -> SystemResult {
    let Some((Initialized(tile_view_pattern), Initialized(raster_resources))) =
        world.resources.query_mut::<(
            &mut Eventually<WgpuTileViewPattern>,
            &mut Eventually<RasterResources>,
        )>()
    else {
        return Err(SystemError::Dependencies);
    };
    for layer in style.layers.iter().filter(|layer| layer.type_ == "raster") {
        let mut uniforms = match &layer.paint {
            Some(LayerPaint::Raster(paint)) => {
                RasterUniforms::from_paint(paint, view_state.zoom().value())
            }
            _ => RasterUniforms::from_paint(&RasterPaint::default(), 0.0),
        };
        uniforms.align = crate::raster::paint::pixel_alignment(view_state);
        raster_resources.write_layer_paint(device, queue, &layer.id, &uniforms);
    }

    let mut items = Vec::new();
    let uses_globe = style.projection.as_ref().is_some_and(|specification| {
        specification
            .projection_type
            .uses_globe_rendering(view_state.zoom().value())
    });

    for view_tile in tile_view_pattern.iter() {
        let coords = &view_tile.coords();
        tracing::trace!("Drawing tile at {coords}");

        for style_layer in style.layers.iter().filter(|layer| {
            layer.type_ == "raster" && layer.is_visible_at(view_state.zoom().value())
        }) {
            let Some(source) = raster_resources.layer_source(&style_layer.id) else {
                continue;
            };
            view_tile.render_raster_source(source, |source_shape| {
                if raster_resources
                    .layer_texture(&style_layer.id, &source_shape.coords())
                    .is_none()
                {
                    return;
                }
                items.push(source_draws(source_shape, style_layer, uses_globe));
            });
        }
    }

    let Some((layer_item_phase, tile_mask_phase)) = world
        .resources
        .query_mut::<(&mut RenderPhase<LayerItem>, &mut RenderPhase<TileMaskItem>)>()
    else {
        return Err(SystemError::Dependencies);
    };

    for (layers, masks) in items {
        for layer in layers {
            layer_item_phase.add(layer);
        }
        for mask in masks {
            tile_mask_phase.add(mask);
        }
    }

    Ok(())
}

fn source_draws(
    source_shape: &crate::render::tile_view_pattern::TileShape,
    style_layer: &crate::style::layer::StyleLayer,
    uses_globe: bool,
) -> (Vec<LayerItem>, Vec<TileMaskItem>) {
    let mut masks = Vec::with_capacity(2);
    if uses_globe {
        masks.push(TileMaskItem {
            projection: ProjectionBinding::View,
            draw_function: Box::new(DrawState::<TileMaskItem, DrawMasks>::new()),
            source_shape: source_shape.clone(),
            generate_borders: true,
        });
    }
    masks.push(TileMaskItem {
        projection: ProjectionBinding::View,
        draw_function: Box::new(DrawState::<TileMaskItem, DrawMasks>::new()),
        source_shape: source_shape.clone(),
        generate_borders: false,
    });
    let mut layers = Vec::with_capacity(if uses_globe { 2 } else { 1 });
    if uses_globe {
        layers.push(LayerItem {
            projection: ProjectionBinding::View,
            draw_function: Box::new(DrawState::<LayerItem, DrawRasterTiles>::new()),
            index: style_layer.index,
            generate_borders: true,
            style_layer: style_layer.id.clone(),
            tile: Tile {
                coords: source_shape.coords(),
            },
            source_shape: source_shape.clone(),
        });
    }
    layers.push(LayerItem {
        projection: ProjectionBinding::View,
        draw_function: Box::new(DrawState::<LayerItem, DrawRasterTiles>::new()),
        index: style_layer.index,
        generate_borders: !uses_globe,
        style_layer: style_layer.id.clone(),
        tile: Tile {
            coords: source_shape.coords(),
        },
        source_shape: source_shape.clone(),
    });
    (layers, masks)
}
