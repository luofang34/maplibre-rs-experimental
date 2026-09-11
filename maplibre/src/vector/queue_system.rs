//! Queues [PhaseItems](crate::render::render_phase::PhaseItem) for rendering.
use crate::{
    context::MapContext,
    io::tile_sources::TileKind,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        render_commands::DrawMasks,
        render_phase::{DrawState, LayerItem, ProjectionBinding, RenderPhase, TileMaskItem},
        tile_view_pattern::WgpuTileViewPattern,
    },
    tcs::{
        system::{SystemError, SystemResult},
        tiles::Tile,
    },
    vector::{
        render_commands::{DrawCircleTiles, DrawLineTiles, DrawVectorTiles},
        VectorBufferPool,
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
    let Some((
        Initialized(tile_view_pattern),
        Initialized(buffer_pool),
        mask_phase,
        layer_item_phase,
    )) = world.resources.query_mut::<(
        &mut Eventually<WgpuTileViewPattern>,
        &mut Eventually<VectorBufferPool>,
        &mut RenderPhase<TileMaskItem>,
        &mut RenderPhase<LayerItem>,
    )>()
    else {
        return Err(SystemError::Dependencies);
    };

    let buffer_pool_index = buffer_pool.index();
    let zoom = view_state.zoom().value();
    let uses_globe = style
        .projection
        .as_ref()
        .is_some_and(|specification| specification.projection_type.uses_globe_rendering(zoom));

    for view_tile in tile_view_pattern.iter() {
        let coords = &view_tile.coords();
        tracing::trace!("Drawing tile at {coords}");

        // draw tile normal or the source e.g. parent or children
        view_tile.render_kind(TileKind::Vector, |source_shape| {
            if uses_globe {
                mask_phase.add(TileMaskItem {
                    projection: ProjectionBinding::View,
                    draw_function: Box::new(DrawState::<TileMaskItem, DrawMasks>::new()),
                    source_shape: source_shape.clone(),
                    generate_borders: true,
                });
            }
            mask_phase.add(TileMaskItem {
                projection: ProjectionBinding::View,
                draw_function: Box::new(DrawState::<TileMaskItem, DrawMasks>::new()),
                source_shape: source_shape.clone(),
                generate_borders: false,
            });

            if let Some(layer_entries) = buffer_pool_index.get_layers(source_shape.coords()) {
                for layer_entry in layer_entries {
                    if !layer_entry
                        .style_layer
                        .is_visible_at(view_state.style_zoom().value())
                    {
                        continue;
                    }
                    let is_line = layer_entry.style_layer.type_ == "line";
                    let draw_function: Box<dyn crate::render::render_phase::Draw<LayerItem>> =
                        match layer_entry.style_layer.type_.as_str() {
                            "line" => Box::new(DrawState::<LayerItem, DrawLineTiles>::new()),
                            "circle" => Box::new(DrawState::<LayerItem, DrawCircleTiles>::new()),
                            _ => Box::new(DrawState::<LayerItem, DrawVectorTiles>::new()),
                        };

                    layer_item_phase.add(LayerItem {
                        projection: ProjectionBinding::View,
                        draw_function,
                        index: layer_entry.style_layer.index,
                        is_line,
                        generate_borders: false,
                        style_layer: layer_entry.style_layer.id.clone(),
                        tile: Tile {
                            coords: layer_entry.coords,
                        },
                        source_shape: source_shape.clone(),
                    });
                }
            }
        });
    }

    Ok(())
}
