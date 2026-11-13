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
            DrawExtrusionPatternColor, DrawExtrusionTranslucentBack, DrawExtrusionTranslucentFront,
            DrawLineTiles, DrawPatternTiles, DrawVectorTiles,
        },
        VectorBufferPool,
    },
};

pub fn queue_system(
    MapContext {
        style,
        view_state,
        world,
        renderer,
        ..
    }: &mut MapContext,
) -> SystemResult {
    if renderer.settings.buffer_pools.is_empty() {
        return Ok(());
    }
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
    let translucent_extrusion_layers: std::collections::HashSet<&str> = style
        .layers
        .iter()
        .filter(|layer| {
            matches!(
                &layer.paint,
                Some(crate::style::layer::LayerPaint::FillExtrusion(paint))
                    if paint.fill_extrusion_translucent == Some(true)
            )
        })
        .map(|layer| layer.id.as_str())
        .collect();
    let unclipped_3d_layers: std::collections::HashSet<&str> = style
        .layers
        .iter()
        .filter(|layer| {
            translucent_extrusion_layers.contains(layer.id.as_str())
                || matches!(
                    super::structures::kind(layer),
                    Some(super::structures::StructureKind::ElevatedLine)
                )
        })
        .map(|layer| layer.id.as_str())
        .collect();
    let style_zoom = view_state.style_zoom().value();
    let mut active_unclipped_3d_shapes: std::collections::HashSet<(
        i32,
        crate::coords::WorldTileCoords,
        u32,
    )> = std::collections::HashSet::new();
    for view_tile in tile_view_pattern.iter() {
        view_tile.render_kind(TileKind::Vector, |shape| {
            if let Some(layer_entries) = buffer_pool_index.get_layers(shape.coords()) {
                for layer_entry in layer_entries {
                    if unclipped_3d_layers.contains(layer_entry.style_layer.id.as_str())
                        && layer_entry.style_layer.is_visible_at(style_zoom)
                    {
                        active_unclipped_3d_shapes.insert((
                            shape.wrap,
                            shape.coords(),
                            layer_entry.style_layer.index,
                        ));
                    }
                }
            }
        });
    }
    // Every tile's depth is drawn before any tile's colour, so an extrusion that crosses tiles
    // shows its nearest surface once instead of once per tile.
    let mut extrusion_colors = Vec::new();
    let mut extrusion_clears = Vec::new();
    let mut translucent_fronts = Vec::new();
    let mut queued_unclipped_3d: std::collections::HashSet<(
        i32,
        crate::coords::WorldTileCoords,
        u32,
    )> = std::collections::HashSet::new();
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
                    if unclipped_3d_layers.contains(layer_entry.style_layer.id.as_str())
                        && (has_active_ancestor(
                            &active_unclipped_3d_shapes,
                            source_shape.wrap,
                            source_shape.coords(),
                            layer_entry.style_layer.index,
                        ) || !queued_unclipped_3d.insert((
                            source_shape.wrap,
                            source_shape.coords(),
                            layer_entry.style_layer.index,
                        )))
                    {
                        continue;
                    }
                    let draw_function: Box<dyn crate::render::render_phase::Draw<LayerItem>> =
                        match layer_entry.style_layer.type_.as_str() {
                            // The heatmap plugin draws these into a density target instead.
                            "heatmap" => continue,
                            "line" => Box::new(DrawState::<LayerItem, DrawLineTiles>::new()),
                            "circle" => Box::new(DrawState::<LayerItem, DrawCircleTiles>::new()),
                            "fill-extrusion"
                                if translucent_extrusion_layers
                                    .contains(layer_entry.style_layer.id.as_str()) =>
                            {
                                translucent_fronts.push(LayerItem {
                                    projection: ProjectionBinding::View,
                                    draw_function: Box::new(DrawState::<
                                        LayerItem,
                                        DrawExtrusionTranslucentFront,
                                    >::new()),
                                    index: layer_entry.style_layer.index,
                                    generate_borders: false,
                                    style_layer: layer_entry.style_layer.id.clone(),
                                    tile: Tile {
                                        coords: layer_entry.coords,
                                    },
                                    source_shape: source_shape.clone(),
                                    run: None,
                                });
                                Box::new(DrawState::<LayerItem, DrawExtrusionTranslucentBack>::new())
                            }
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

    for item in extrusion_colors
        .into_iter()
        .chain(extrusion_clears)
        .chain(translucent_fronts)
    {
        layer_item_phase.add(item);
    }

    Ok(())
}

fn has_active_ancestor(
    active: &std::collections::HashSet<(i32, crate::coords::WorldTileCoords, u32)>,
    wrap: i32,
    coords: crate::coords::WorldTileCoords,
    layer_index: u32,
) -> bool {
    let mut current = coords.get_parent();
    while let Some(ancestor) = current {
        if active.contains(&(wrap, ancestor, layer_index)) {
            return true;
        }
        current = ancestor.get_parent();
    }
    false
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::has_active_ancestor;
    use crate::coords::{WorldTileCoords, ZoomLevel};

    #[test]
    fn ancestor_without_matching_3d_layer_never_blanks_descendant_airspace() {
        let overview = WorldTileCoords {
            x: 0,
            y: 0,
            z: ZoomLevel::new(0),
        };
        let parent_z8 = WorldTileCoords {
            x: 75,
            y: 96,
            z: ZoomLevel::new(8),
        };
        let child_z10 = WorldTileCoords {
            x: 300,
            y: 384,
            z: ZoomLevel::new(10),
        };
        let mut active = HashSet::new();
        // Overview 0/0/0 only holds layer 1 (e.g. coastline/ice), NOT layer 9 (3D airspace).
        active.insert((0, overview, 1));
        assert!(!has_active_ancestor(&active, 0, child_z10, 9));

        // When parent z8 actively draws the same 3D layer 9, child z10 yields to avoid double alpha.
        active.insert((0, parent_z8, 9));
        assert!(has_active_ancestor(&active, 0, child_z10, 9));
    }
}
