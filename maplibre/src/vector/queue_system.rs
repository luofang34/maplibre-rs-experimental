//! Queues [PhaseItems](crate::render::render_phase::PhaseItem) for rendering.
use phase_cache::{CachedVectorPhases, VectorPhaseCacheKey, VectorQueueScratch};
use rustc_hash::FxHashSet;

use crate::{
    context::MapContext,
    io::tile_sources::TileKind,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        render_commands::DrawMasks,
        render_phase::{
            DrawState, LayerItem, MaskGroup, ProjectionBinding, RenderPhase, SortedRun,
            TileMaskItem, UNSET_ENTRY_SLOT,
        },
        tile_view_pattern::WgpuTileViewPattern,
    },
    tcs::{
        system::{SystemError, SystemResult},
        tiles::Tile,
    },
    vector::{
        render_commands::{
            DrawCircleTiles, DrawExtrusionColor, DrawExtrusionDepth, DrawExtrusionPatternColor,
            DrawExtrusionStencilClear, DrawExtrusionTranslucentBack, DrawExtrusionTranslucentFront,
            DrawLineTiles, DrawPatternTiles, DrawVectorTiles,
        },
        VectorBufferPool,
    },
};

mod phase_cache;

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
    let _ = world.resources.get_or_init_mut::<VectorQueueScratch>();
    let Some((
        Initialized(tile_view_pattern),
        Initialized(buffer_pool),
        mask_phase,
        layer_item_phase,
        scratch,
    )) = world.resources.query_mut::<(
        &mut Eventually<WgpuTileViewPattern>,
        &mut Eventually<VectorBufferPool>,
        &mut RenderPhase<TileMaskItem>,
        &mut RenderPhase<LayerItem>,
        &mut VectorQueueScratch,
    )>()
    else {
        return Err(SystemError::Dependencies);
    };

    let zoom = view_state.zoom().value();
    let style_zoom = view_state.style_zoom().value();
    let uses_globe = style
        .projection
        .as_ref()
        .is_some_and(|specification| specification.projection_type.uses_globe_rendering(zoom));

    let layer_state_digest = scratch.sync_style_sets(style, style_zoom);
    let cache_key = VectorPhaseCacheKey {
        topology_gen: tile_view_pattern.topology_generation(),
        buffer_pool_rev: buffer_pool.revision(),
        style_rev: style.revision(),
        layer_state_digest,
        uses_globe,
    };
    if let Some(cached) = &scratch.cached_phases {
        if cached.key == cache_key {
            mask_phase.extend_from_slice(&cached.masks);
            layer_item_phase.extend_from_slice(&cached.layers);
            return Ok(());
        }
    }

    let mask_start = mask_phase.size();
    let layer_start = layer_item_phase.size();
    let buffer_pool_index = buffer_pool.index();
    let VectorQueueScratch {
        pattern_layers,
        missing_image_layers,
        translucent_extrusion_layers,
        unclipped_3d_layers,
        zero_opacity_layers,
        active_unclipped_3d_shapes,
        queued_unclipped_3d,
        extrusion_colors,
        extrusion_clears,
        translucent_fronts,
        ..
    } = scratch;
    active_unclipped_3d_shapes.clear();
    for view_tile in tile_view_pattern.iter() {
        view_tile.render_kind(TileKind::Vector, |shape| {
            if let Some(layer_entries) = buffer_pool_index.get_layers(shape.coords()) {
                for layer_entry in layer_entries {
                    if unclipped_3d_layers.contains(&layer_entry.style_layer_id)
                        && !zero_opacity_layers.contains(&layer_entry.style_layer_id)
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
    extrusion_colors.clear();
    extrusion_clears.clear();
    translucent_fronts.clear();
    queued_unclipped_3d.clear();
    for view_tile in tile_view_pattern.iter() {
        let coords = &view_tile.coords();
        tracing::trace!("Drawing tile at {coords}");

        // draw tile normal or the source e.g. parent or children
        view_tile.render_kind(TileKind::Vector, |source_shape| {
            let shape_data = source_shape.draw_data();
            if uses_globe {
                mask_phase.add(TileMaskItem {
                    projection: ProjectionBinding::View,
                    draw_function: &DrawState::<TileMaskItem, DrawMasks>::INSTANCE,
                    source_shape: shape_data,
                    generate_borders: true,
                    is_vector: true,
                });
            }
            mask_phase.add(TileMaskItem {
                projection: ProjectionBinding::View,
                draw_function: &DrawState::<TileMaskItem, DrawMasks>::INSTANCE,
                source_shape: shape_data,
                generate_borders: false,
                is_vector: true,
            });

            if let Some(layer_entries) = buffer_pool_index.get_layers(source_shape.coords()) {
                for (slot_idx, layer_entry) in layer_entries.iter().enumerate() {
                    if zero_opacity_layers.contains(&layer_entry.style_layer_id)
                        || missing_image_layers.contains(&layer_entry.style_layer_id)
                        || !layer_entry.style_layer.is_visible_at(style_zoom)
                    {
                        continue;
                    }
                    let layer_id = layer_entry.style_layer_id;
                    let entry_slot = u16::try_from(slot_idx).unwrap_or(UNSET_ENTRY_SLOT);
                    let is_unclipped_3d = unclipped_3d_layers.contains(&layer_id);
                    if is_unclipped_3d
                        && (has_active_ancestor(
                            active_unclipped_3d_shapes,
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
                    let mask_group = if is_unclipped_3d {
                        MaskGroup::Unmasked
                    } else {
                        MaskGroup::Vector
                    };
                    let draw_function: &'static dyn crate::render::render_phase::Draw<LayerItem> =
                        match layer_entry.style_layer.type_.as_str() {
                            // The heatmap plugin draws these into a density target instead.
                            "heatmap" => continue,
                            "line" => &DrawState::<LayerItem, DrawLineTiles>::INSTANCE,
                            "circle" => &DrawState::<LayerItem, DrawCircleTiles>::INSTANCE,
                            "fill-extrusion"
                                if translucent_extrusion_layers.contains(&layer_id) =>
                            {
                                translucent_fronts.push(LayerItem {
                                    projection: ProjectionBinding::View,
                                    draw_function: &DrawState::<
                                        LayerItem,
                                        DrawExtrusionTranslucentFront,
                                    >::INSTANCE,
                                    index: layer_entry.style_layer.index,
                                    generate_borders: false,
                                    style_layer: layer_id,
                                    entry_slot,
                                    tile: Tile {
                                        coords: layer_entry.coords,
                                    },
                                    source_shape: shape_data,
                                    mask_group: MaskGroup::Unmasked,
                                    run: None,
                                });
                                &DrawState::<LayerItem, DrawExtrusionTranslucentBack>::INSTANCE
                            }
                            "fill-extrusion" => {
                                if !extrusion_clears
                                    .iter()
                                    .any(|item| item.style_layer == layer_id)
                                {
                                    extrusion_clears.push(LayerItem {
                                        projection: ProjectionBinding::View,
                                        draw_function: &DrawState::<
                                            LayerItem,
                                            DrawExtrusionStencilClear,
                                        >::INSTANCE,
                                        index: layer_entry.style_layer.index,
                                        generate_borders: false,
                                        style_layer: layer_id,
                                        entry_slot: UNSET_ENTRY_SLOT,
                                        tile: Tile {
                                            coords: layer_entry.coords,
                                        },
                                        source_shape: shape_data,
                                        mask_group: MaskGroup::Vector,
                                        run: None,
                                    });
                                }
                                let color: &'static dyn crate::render::render_phase::Draw<
                                    LayerItem,
                                > = if pattern_layers.contains(&layer_id) {
                                    &DrawState::<LayerItem, DrawExtrusionPatternColor>::INSTANCE
                                } else {
                                    &DrawState::<LayerItem, DrawExtrusionColor>::INSTANCE
                                };
                                extrusion_colors.push(LayerItem {
                                    projection: ProjectionBinding::View,
                                    draw_function: color,
                                    index: layer_entry.style_layer.index,
                                    generate_borders: false,
                                    style_layer: layer_id,
                                    entry_slot,
                                    tile: Tile {
                                        coords: layer_entry.coords,
                                    },
                                    source_shape: shape_data,
                                    mask_group: MaskGroup::Vector,
                                    run: None,
                                });
                                &DrawState::<LayerItem, DrawExtrusionDepth>::INSTANCE
                            }
                            "fill" if pattern_layers.contains(&layer_id) => {
                                &DrawState::<LayerItem, DrawPatternTiles>::INSTANCE
                            }
                            _ => &DrawState::<LayerItem, DrawVectorTiles>::INSTANCE,
                        };

                    // Features that draw in the order of a sort key, across tiles, draw a run at
                    // a time.
                    let runs = (layer_entry.style_layer.type_ == "circle")
                        .then(|| buffer_pool.sort_runs_by_id(layer_entry.coords, layer_id))
                        .flatten();
                    if let Some(runs) = runs.filter(|runs| !runs.is_empty()) {
                        for (key, range) in runs {
                            layer_item_phase.add(LayerItem {
                                projection: ProjectionBinding::View,
                                draw_function: &DrawState::<LayerItem, DrawCircleTiles>::INSTANCE,
                                index: layer_entry.style_layer.index,
                                generate_borders: false,
                                style_layer: layer_id,
                                entry_slot,
                                tile: Tile {
                                    coords: layer_entry.coords,
                                },
                                source_shape: shape_data,
                                mask_group: MaskGroup::Vector,
                                run: Some(SortedRun {
                                    key: *key,
                                    start: range.start,
                                    end: range.end,
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
                        style_layer: layer_id,
                        entry_slot,
                        tile: Tile {
                            coords: layer_entry.coords,
                        },
                        source_shape: shape_data,
                        mask_group,
                        run: None,
                    });
                }
            }
        });
    }

    for item in extrusion_colors
        .drain(..)
        .chain(extrusion_clears.drain(..))
        .chain(translucent_fronts.drain(..))
    {
        layer_item_phase.add(item);
    }

    mask_phase.sort_tail(mask_start);
    layer_item_phase.sort_tail(layer_start);
    scratch.cached_phases = Some(CachedVectorPhases {
        key: cache_key,
        masks: mask_phase.items()[mask_start..].to_vec(),
        layers: layer_item_phase.items()[layer_start..].to_vec(),
    });

    Ok(())
}

fn has_active_ancestor(
    active: &FxHashSet<(i32, crate::coords::WorldTileCoords, u32)>,
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
    use rustc_hash::FxHashSet;

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
        let mut active = FxHashSet::default();
        // Overview 0/0/0 only holds layer 1 (e.g. coastline/ice), NOT layer 9 (3D airspace).
        active.insert((0, overview, 1));
        assert!(!has_active_ancestor(&active, 0, child_z10, 9));

        // When parent z8 actively draws the same 3D layer 9, child z10 yields to avoid double alpha.
        active.insert((0, parent_z8, 9));
        assert!(has_active_ancestor(&active, 0, child_z10, 9));
    }

    #[test]
    fn scratch_style_sets_reuse_cache_and_refresh_on_revision_or_zoom() {
        let mut style: crate::style::Style = serde_json::from_str(
            r#"{
                "version": 8,
                "sources": { "v": { "type": "vector", "tiles": ["https://example.invalid/{z}/{x}/{y}.pbf"] } },
                "layers": [
                    { "id": "hidden", "type": "fill", "source": "v", "source-layer": "s", "paint": { "fill-opacity": 0.0 } },
                    { "id": "glass", "type": "fill-extrusion", "source": "v", "source-layer": "s", "paint": { "fill-extrusion-translucent": true } }
                ]
            }"#,
        )
        .expect("valid style");
        let mut scratch = super::VectorQueueScratch::default();
        scratch.sync_style_sets(&style, 12.0);
        assert!(scratch.zero_opacity_layers.contains(&"hidden".into()));
        assert!(scratch
            .translucent_extrusion_layers
            .contains(&"glass".into()));
        assert!(scratch.unclipped_3d_layers.contains(&"glass".into()));

        style.layers[0].paint = Some(crate::style::layer::LayerPaint::Fill(Default::default()));
        // Without bumping revision or changing zoom, cached sets remain untouched.
        scratch.sync_style_sets(&style, 12.0);
        assert!(scratch.zero_opacity_layers.contains(&"hidden".into()));

        // Bumping style revision refreshes the cached sets in a single pass.
        style.bump_revision();
        let d_after = scratch.sync_style_sets(&style, 12.0);
        assert!(!scratch.zero_opacity_layers.contains(&"hidden".into()));
        assert_ne!(d_after, 0);
    }

    #[test]
    fn digest_stays_constant_within_zoom_band_and_changes_on_transition() {
        let style: crate::style::Style = serde_json::from_str(
            r#"{
                "version": 8,
                "sources": { "v": { "type": "vector", "tiles": ["https://example.invalid/{z}/{x}/{y}.pbf"] } },
                "layers": [
                    { "id": "roads", "type": "line", "source": "v", "source-layer": "road", "minzoom": 10, "maxzoom": 16 }
                ]
            }"#,
        )
        .expect("valid style");
        let mut scratch = super::VectorQueueScratch::default();
        let d1 = scratch.sync_style_sets(&style, 12.15);
        let d2 = scratch.sync_style_sets(&style, 13.75);
        let d3 = scratch.sync_style_sets(&style, 9.5);
        assert_eq!(d1, d2);
        assert_ne!(d1, d3);
    }
}
