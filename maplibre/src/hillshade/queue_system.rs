//! Queues one draw per DEM-shaded layer and raster tile shape, as the raster queue does.

use crate::{
    context::MapContext,
    hillshade::{dem_layer_kind, render_commands::DrawDemTiles, resources::HillshadeResources},
    io::tile_sources::TileKind,
    raster::resource::RasterResources,
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
        Initialized(raster_resources),
        Initialized(resources),
    )) = world.resources.query::<(
        &Eventually<WgpuTileViewPattern>,
        &Eventually<RasterResources>,
        &Eventually<HillshadeResources>,
    )>()
    else {
        return Err(SystemError::Dependencies);
    };
    let zoom = view_state.zoom().value();
    let uses_globe = style
        .projection
        .as_ref()
        .is_some_and(|specification| specification.projection_type.uses_globe_rendering(zoom));
    let layers: Vec<_> = style
        .layers
        .iter()
        .filter(|layer| {
            dem_layer_kind(&layer.type_).is_some()
                && layer.is_visible_at(zoom)
                && resources.layer(&layer.id).is_some()
        })
        .collect();
    if layers.is_empty() {
        return Ok(());
    }

    let mut items = Vec::new();
    for view_tile in tile_view_pattern.iter() {
        view_tile.render_kind(TileKind::Raster, |source_shape| {
            if raster_resources
                .get_bound_texture(&source_shape.coords())
                .is_none()
            {
                return;
            }
            for style_layer in &layers {
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
                let mut draws = Vec::with_capacity(2);
                if uses_globe {
                    draws.push(LayerItem {
                        projection: ProjectionBinding::View,
                        draw_function: Box::new(DrawState::<LayerItem, DrawDemTiles>::new()),
                        index: style_layer.index,
                        is_line: false,
                        generate_borders: true,
                        style_layer: style_layer.id.clone(),
                        tile: Tile {
                            coords: source_shape.coords(),
                        },
                        source_shape: source_shape.clone(),
                    });
                }
                // The seam-expanding mesh would draw the tile edges twice, which shows through
                // translucent shading; the flat map has no cracks to hide, as in GL JS.
                draws.push(LayerItem {
                    projection: ProjectionBinding::View,
                    draw_function: Box::new(DrawState::<LayerItem, DrawDemTiles>::new()),
                    index: style_layer.index,
                    is_line: false,
                    generate_borders: false,
                    style_layer: style_layer.id.clone(),
                    tile: Tile {
                        coords: source_shape.coords(),
                    },
                    source_shape: source_shape.clone(),
                });
                items.push((draws, masks));
            }
        });
    }

    let Some((layer_item_phase, tile_mask_phase)) = world
        .resources
        .query_mut::<(&mut RenderPhase<LayerItem>, &mut RenderPhase<TileMaskItem>)>()
    else {
        return Err(SystemError::Dependencies);
    };
    for (draws, masks) in items {
        for draw in draws {
            layer_item_phase.add(draw);
        }
        for mask in masks {
            tile_mask_phase.add(mask);
        }
    }
    Ok(())
}
