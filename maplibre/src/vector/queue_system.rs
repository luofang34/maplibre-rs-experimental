//! Queues [PhaseItems](crate::render::render_phase::PhaseItem) for rendering.
use crate::{
    context::MapContext,
    io::tile_sources::TileKind,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        render_commands::DrawMasks,
        render_phase::{
            DrawState, LayerItem, ProjectionBinding, RenderPhase, SortedRun, TileMaskItem,
        },
        tile_view_pattern::WgpuTileViewPattern,
    },
    tcs::{
        system::{SystemError, SystemResult},
        tiles::Tile,
    },
    vector::{
        render_commands::{
            DrawCircleTiles, DrawExtrusionClear, DrawExtrusionColor, DrawExtrusionDepth,
            DrawExtrusionPatternColor, DrawLineTiles, DrawPatternTiles, DrawVectorTiles,
        },
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

    let pattern_layers: std::collections::HashSet<&str> = style
        .layers
        .iter()
        .filter(|layer| {
            layer.paint.as_ref().is_some_and(|paint| {
                super::pattern::pattern_name(paint, view_state.style_zoom().value()).is_some()
                    || super::pattern::per_feature_pattern(paint).is_some()
            })
        })
        .map(|layer| layer.id.as_str())
        .collect();
    // Every tile's depth is drawn before any tile's colour, so an extrusion that crosses tiles
    // shows its nearest surface once instead of once per tile.
    let mut extrusion_colors = Vec::new();
    let mut extrusion_clears = Vec::new();
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
                    if super::pattern::names_missing_image(
                        layer_entry.style_layer.paint.as_ref(),
                        style,
                        view_state.style_zoom().value(),
                    ) {
                        continue;
                    }
                    let draw_function: Box<dyn crate::render::render_phase::Draw<LayerItem>> =
                        match layer_entry.style_layer.type_.as_str() {
                            // The heatmap plugin draws these into a density target instead.
                            "heatmap" => continue,
                            "line" => Box::new(DrawState::<LayerItem, DrawLineTiles>::new()),
                            "circle" => Box::new(DrawState::<LayerItem, DrawCircleTiles>::new()),
                            "fill-extrusion" => {
                                extrusion_clears.push(LayerItem {
                                    projection: ProjectionBinding::View,
                                    draw_function: Box::new(DrawState::<
                                        LayerItem,
                                        DrawExtrusionClear,
                                    >::new(
                                    )),
                                    index: layer_entry.style_layer.index,
                                    generate_borders: false,
                                    style_layer: layer_entry.style_layer.id.clone(),
                                    tile: Tile {
                                        coords: layer_entry.coords,
                                    },
                                    source_shape: source_shape.clone(),
                                    run: None,
                                });
                                let color: Box<dyn crate::render::render_phase::Draw<LayerItem>> =
                                    if pattern_layers.contains(layer_entry.style_layer.id.as_str())
                                    {
                                        Box::new(
                                            DrawState::<LayerItem, DrawExtrusionPatternColor>::new(
                                            ),
                                        )
                                    } else {
                                        Box::new(DrawState::<LayerItem, DrawExtrusionColor>::new())
                                    };
                                extrusion_colors.push(LayerItem {
                                    projection: ProjectionBinding::View,
                                    draw_function: color,
                                    index: layer_entry.style_layer.index,
                                    generate_borders: false,
                                    style_layer: layer_entry.style_layer.id.clone(),
                                    tile: Tile {
                                        coords: layer_entry.coords,
                                    },
                                    source_shape: source_shape.clone(),
                                    run: None,
                                });
                                Box::new(DrawState::<LayerItem, DrawExtrusionDepth>::new())
                            }
                            "fill"
                                if pattern_layers.contains(layer_entry.style_layer.id.as_str()) =>
                            {
                                Box::new(DrawState::<LayerItem, DrawPatternTiles>::new())
                            }
                            _ => Box::new(DrawState::<LayerItem, DrawVectorTiles>::new()),
                        };

                    // Features that draw in the order of a sort key, across tiles, draw a run at
                    // a time.
                    let runs = (layer_entry.style_layer.type_ == "circle")
                        .then(|| {
                            buffer_pool.sort_runs(layer_entry.coords, &layer_entry.style_layer.id)
                        })
                        .flatten();
                    if let Some(runs) = runs.filter(|runs| !runs.is_empty()) {
                        for (key, range) in runs {
                            layer_item_phase.add(LayerItem {
                                projection: ProjectionBinding::View,
                                draw_function: Box::new(
                                    DrawState::<LayerItem, DrawCircleTiles>::new(),
                                ),
                                index: layer_entry.style_layer.index,
                                generate_borders: false,
                                style_layer: layer_entry.style_layer.id.clone(),
                                tile: Tile {
                                    coords: layer_entry.coords,
                                },
                                source_shape: source_shape.clone(),
                                run: Some(SortedRun {
                                    key: *key,
                                    range: range.clone(),
                                }),
                            });
                        }
                        continue;
                    }
                    layer_item_phase.add(LayerItem {
                        projection: ProjectionBinding::View,
                        draw_function,
                        index: layer_entry.style_layer.index,
                        generate_borders: false,
                        style_layer: layer_entry.style_layer.id.clone(),
                        tile: Tile {
                            coords: layer_entry.coords,
                        },
                        source_shape: source_shape.clone(),
                        run: None,
                    });
                }
            }
        });
    }

    for item in extrusion_colors.into_iter().chain(extrusion_clears) {
        layer_item_phase.add(item);
    }

    Ok(())
}
