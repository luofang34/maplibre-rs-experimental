//! Queues [PhaseItems](crate::render::render_phase::PhaseItem) for rendering.

use crate::{
    context::MapContext,
    raster::{
        cross_fade::RasterCrossFade, paint::RasterUniforms, render_commands::DrawRasterTiles,
        resource::RasterResources, RasterLayersDataComponent,
    },
    render::{
        eventually::{Eventually, Eventually::Initialized},
        render_commands::DrawMasks,
        render_phase::{
            DrawState, LayerItem, MaskGroup, ProjectionBinding, RenderCommand, RenderCommandResult,
            RenderPhase, TileMaskItem,
        },
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
    if !style.cached_has_raster_layer() {
        return Ok(());
    }
    let style_zoom = view_state.style_zoom().value();
    let now = instant::Instant::now();
    let mut animating_fade = false;
    if let Some(fade) = world.resources.get_mut::<RasterCrossFade>() {
        animating_fade = fade.advance(now);
    }
    let departing_opacity: rustc_hash::FxHashMap<String, f32> = world
        .resources
        .get::<RasterCrossFade>()
        .map(|fade| {
            style
                .layers
                .iter()
                .filter_map(|layer| {
                    let departing = fade.departing(layer.source.as_deref())?;
                    Some((layer.id.clone(), departing.opacity))
                })
                .collect()
        })
        .unwrap_or_default();
    let Some((tile_view_pattern, raster_resources)) = world.resources.query_mut::<(
        &mut Eventually<WgpuTileViewPattern>,
        &mut Eventually<RasterResources>,
    )>() else {
        return Err(SystemError::Dependencies);
    };
    let (Initialized(tile_view_pattern), Initialized(raster_resources)) =
        (tile_view_pattern, raster_resources)
    else {
        return Ok(());
    };
    for layer in style
        .layers
        .iter()
        .filter(|layer| layer.type_ == "raster" && layer.is_visible_at(style_zoom))
    {
        let mut uniforms = match &layer.paint {
            Some(LayerPaint::Raster(paint)) => RasterUniforms::from_paint(paint, style_zoom),
            _ => RasterUniforms::from_paint(&RasterPaint::default(), 0.0),
        };
        uniforms.align = crate::raster::paint::pixel_alignment(view_state);
        raster_resources.write_layer_paint(device, queue, &layer.id, &uniforms);
        if let Some(opacity) = departing_opacity.get(&layer.id) {
            uniforms.opacity *= opacity;
            raster_resources.write_departing_paint(device, queue, &layer.id, &uniforms);
        }
    }

    let mut items = Vec::new();
    let uses_globe = style.projection.as_ref().is_some_and(|specification| {
        specification
            .projection_type
            .uses_globe_rendering(view_state.zoom().value())
    });

    let mut visible_by_source: rustc_hash::FxHashMap<
        String,
        (u32, rustc_hash::FxHashSet<crate::coords::WorldTileCoords>),
    > = rustc_hash::FxHashMap::default();
    for view_tile in tile_view_pattern.iter() {
        let coords = &view_tile.coords();
        tracing::trace!("Drawing tile at {coords}");

        for style_layer in style
            .layers
            .iter()
            .filter(|layer| layer.type_ == "raster" && layer.is_visible_at(style_zoom))
        {
            let Some(source) = raster_resources.layer_source(&style_layer.id) else {
                continue;
            };
            let default_fade = if style.terrain.is_some() {
                0
            } else {
                crate::raster::cross_fade::DEFAULT_RASTER_FADE_DURATION_MS
            };
            let fade_duration = match &style_layer.paint {
                Some(LayerPaint::Raster(paint)) => {
                    paint.raster_fade_duration.unwrap_or(default_fade)
                }
                _ => default_fade,
            };
            view_tile.render_raster_source(source, |source_shape| {
                if raster_resources
                    .layer_texture(&style_layer.id, &source_shape.coords())
                    .is_none()
                {
                    if !source_shape.is_drawn_within_another()
                        && world
                            .tiles
                            .query::<&RasterLayersDataComponent>(source_shape.coords())
                            .is_some_and(|data| data.has_empty_source(source))
                    {
                        items.push(empty_mask_draws(source_shape, style_layer, uses_globe));
                    }
                    return;
                }
                if !source_shape.is_drawn_within_another() {
                    if let Some(src_name) = style_layer.source.as_deref() {
                        let entry = visible_by_source
                            .entry(src_name.to_owned())
                            .or_insert_with(|| (fade_duration, Default::default()));
                        entry.0 = entry.0.max(fade_duration);
                        entry.1.insert(source_shape.coords());
                    }
                }
                items.push(source_draws(source_shape, style_layer, uses_globe));
            });
        }
    }

    if !visible_by_source.is_empty() {
        if world.resources.get::<RasterCrossFade>().is_none() {
            world.resources.insert(RasterCrossFade::default());
        }
        if let Some(fade) = world.resources.get_mut::<RasterCrossFade>() {
            for (src_name, (duration_ms, current_tiles)) in &visible_by_source {
                fade.update_live_fades(src_name, current_tiles, *duration_ms, now);
            }
            if fade.has_active_timed_fade() {
                animating_fade = true;
            }
        }
    }
    if animating_fade {
        crate::render::frame_signals::keep_animating(world);
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

/// No-op raster draw paired with a stencil mask so intentionally empty (HTTP 204) child tiles
/// occlude parent fallbacks in [`RenderPhase<TileMaskItem>`] without drawing or binding a texture.
struct SkipEmptyRasterDraw;

impl RenderCommand<LayerItem> for SkipEmptyRasterDraw {
    fn render<'w>(
        _world: &'w crate::tcs::world::World,
        _item: &LayerItem,
        _pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        RenderCommandResult::Success
    }
}

fn source_masks(
    source_shape: &crate::render::tile_view_pattern::TileShape,
    uses_globe: bool,
) -> Vec<TileMaskItem> {
    if source_shape.is_drawn_within_another() {
        return Vec::new();
    }
    let shape_data = source_shape.draw_data();
    let mut masks = Vec::with_capacity(if uses_globe { 2 } else { 1 });
    if uses_globe {
        masks.push(TileMaskItem {
            projection: ProjectionBinding::View,
            draw_function: &DrawState::<TileMaskItem, DrawMasks>::INSTANCE,
            source_shape: shape_data,
            generate_borders: true,
            is_vector: false,
        });
    }
    masks.push(TileMaskItem {
        projection: ProjectionBinding::View,
        draw_function: &DrawState::<TileMaskItem, DrawMasks>::INSTANCE,
        source_shape: shape_data,
        generate_borders: false,
        is_vector: false,
    });
    masks
}

fn empty_mask_draws(
    source_shape: &crate::render::tile_view_pattern::TileShape,
    style_layer: &crate::style::layer::StyleLayer,
    uses_globe: bool,
) -> (Vec<LayerItem>, Vec<TileMaskItem>) {
    let masks = source_masks(source_shape, uses_globe);
    let layers = vec![LayerItem {
        projection: ProjectionBinding::View,
        draw_function: &DrawState::<LayerItem, SkipEmptyRasterDraw>::INSTANCE,
        index: style_layer.index,
        generate_borders: false,
        style_layer: style_layer.layer_id(),
        entry_slot: crate::render::render_phase::UNSET_ENTRY_SLOT,
        tile: Tile {
            coords: source_shape.coords(),
        },
        source_shape: source_shape.draw_data(),
        mask_group: MaskGroup::Mutating,
        run: None,
    }];
    (layers, masks)
}

fn source_draws(
    source_shape: &crate::render::tile_view_pattern::TileShape,
    style_layer: &crate::style::layer::StyleLayer,
    uses_globe: bool,
) -> (Vec<LayerItem>, Vec<TileMaskItem>) {
    let shape_data = source_shape.draw_data();
    let layer_id = style_layer.layer_id();
    let masks = source_masks(source_shape, uses_globe);
    let mut layers = Vec::with_capacity(if uses_globe { 2 } else { 1 });
    if uses_globe {
        layers.push(LayerItem {
            projection: ProjectionBinding::View,
            draw_function: &DrawState::<LayerItem, DrawRasterTiles>::INSTANCE,
            index: style_layer.index,
            generate_borders: true,
            style_layer: layer_id,
            entry_slot: crate::render::render_phase::UNSET_ENTRY_SLOT,
            tile: Tile {
                coords: source_shape.coords(),
            },
            source_shape: shape_data,
            mask_group: MaskGroup::Mutating,
            run: None,
        });
    }
    layers.push(LayerItem {
        projection: ProjectionBinding::View,
        draw_function: &DrawState::<LayerItem, DrawRasterTiles>::INSTANCE,
        index: style_layer.index,
        generate_borders: !uses_globe,
        style_layer: layer_id,
        entry_slot: crate::render::render_phase::UNSET_ENTRY_SLOT,
        tile: Tile {
            coords: source_shape.coords(),
        },
        source_shape: shape_data,
        mask_group: MaskGroup::Mutating,
        run: None,
    });
    (layers, masks)
}
