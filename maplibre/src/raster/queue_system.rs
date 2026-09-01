//! Queues [PhaseItems](crate::render::render_phase::PhaseItem) for rendering.

use crate::{
    context::MapContext,
    raster::render_commands::DrawRasterTiles,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        render_commands::DrawMasks,
        render_phase::{DrawState, LayerItem, RenderPhase, TileMaskItem},
        tile_view_pattern::WgpuTileViewPattern,
    },
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
        ..
    }: &mut MapContext,
) -> SystemResult {
    let Some((Initialized(tile_view_pattern),)) = world
        .resources
        .query::<(&Eventually<WgpuTileViewPattern>,)>()
    else {
        return Err(SystemError::Dependencies);
    };

    let mut items = Vec::new();
    let uses_globe = style.projection.as_ref().is_some_and(|specification| {
        specification
            .projection_type
            .uses_globe_rendering(view_state.zoom().value())
    });

    for view_tile in tile_view_pattern.iter() {
        let coords = &view_tile.coords();
        tracing::trace!("Drawing tile at {coords}");

        // draw tile normal or the source e.g. parent or children
        view_tile.render(|source_shape| {
            // FIXME if raster_resources.has_tile(source_shape.coords(), world) {
            let layer = LayerItem {
                draw_function: Box::new(DrawState::<LayerItem, DrawRasterTiles>::new()),
                index: 0,
                is_line: false,
                style_layer: "raster".to_string(),
                tile: Tile {
                    coords: source_shape.coords(),
                },
                source_shape: source_shape.clone(),
            };
            let mut masks = Vec::with_capacity(2);
            if uses_globe {
                masks.push(TileMaskItem {
                    draw_function: Box::new(DrawState::<TileMaskItem, DrawMasks>::new()),
                    source_shape: source_shape.clone(),
                    generate_borders: true,
                });
            }
            masks.push(TileMaskItem {
                draw_function: Box::new(DrawState::<TileMaskItem, DrawMasks>::new()),
                source_shape: source_shape.clone(),
                generate_borders: false,
            });
            items.push((layer, masks));
        });
    }

    let Some((layer_item_phase, tile_mask_phase)) = world
        .resources
        .query_mut::<(&mut RenderPhase<LayerItem>, &mut RenderPhase<TileMaskItem>)>()
    else {
        return Err(SystemError::Dependencies);
    };

    for (layer, masks) in items {
        layer_item_phase.add(layer);
        for mask in masks {
            tile_mask_phase.add(mask);
        }
    }

    Ok(())
}
