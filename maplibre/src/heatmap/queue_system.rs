//! Queues the density draws of every visible heatmap layer and its composite in the main pass.

use crate::{
    context::MapContext,
    heatmap::{
        render_commands::{DrawHeatmapComposite, DrawHeatmapDensityTiles},
        resources::HeatmapResources,
    },
    io::tile_sources::TileKind,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        render_phase::{DrawState, LayerItem, ProjectionBinding, RenderPhase},
        tile_view_pattern::{TileShape, WgpuTileViewPattern},
    },
    style::layer::LayerPaint,
    tcs::{
        system::{SystemError, SystemResult},
        tiles::Tile,
    },
    vector::VectorBufferPool,
};

/// The point draws of one heatmap layer, in the order its density pass runs them.
pub struct DensityLayer {
    /// Style layer whose density target the draws add into.
    pub layer: String,
    /// One draw per source tile with points of the layer.
    pub items: Vec<LayerItem>,
}

/// Every visible heatmap layer's density draws for this frame.
#[derive(Default)]
pub struct HeatmapDensityPhase {
    /// Layers in style order; a layer without points still appears, so its target is cleared.
    pub layers: Vec<DensityLayer>,
}

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
        Initialized(resources),
        density_phase,
        layer_item_phase,
    )) = world.resources.query_mut::<(
        &mut Eventually<WgpuTileViewPattern>,
        &mut Eventually<VectorBufferPool>,
        &mut Eventually<HeatmapResources>,
        &mut HeatmapDensityPhase,
        &mut RenderPhase<LayerItem>,
    )>()
    else {
        return Err(SystemError::Dependencies);
    };
    density_phase.layers.clear();
    let zoom = view_state.style_zoom().value();
    let layers: Vec<_> = style
        .layers
        .iter()
        .filter(|layer| {
            matches!(layer.paint, Some(LayerPaint::Heatmap(_)))
                && layer.is_visible_at(zoom)
                && resources.density_view(&layer.id).is_some()
        })
        .collect();
    if layers.is_empty() {
        return Ok(());
    }

    // Each source tile is drawn once however many view tiles it stands in for while its
    // children load, so a point is never counted twice.
    let mut sources: Vec<TileShape> = Vec::new();
    for view_tile in tile_view_pattern.iter() {
        view_tile.render_kind(TileKind::Vector, |source_shape| {
            if !sources
                .iter()
                .any(|known| known.coords() == source_shape.coords())
            {
                sources.push(source_shape.clone());
            }
        });
    }
    let Some(any_shape) = sources.first().cloned() else {
        return Ok(());
    };

    let pool_index = buffer_pool.index();
    for layer in layers {
        let mut items = Vec::new();
        for source_shape in &sources {
            let Some(entries) = pool_index.get_layers(source_shape.coords()) else {
                continue;
            };
            for entry in entries {
                if entry.style_layer.id != layer.id {
                    continue;
                }
                items.push(LayerItem {
                    projection: ProjectionBinding::View,
                    draw_function: Box::new(DrawState::<LayerItem, DrawHeatmapDensityTiles>::new()),
                    index: layer.index,
                    generate_borders: false,
                    style_layer: layer.id.clone(),
                    tile: Tile {
                        coords: entry.coords,
                    },
                    source_shape: source_shape.clone(),
                });
            }
        }
        density_phase.layers.push(DensityLayer {
            layer: layer.id.clone(),
            items,
        });
        layer_item_phase.add(LayerItem {
            projection: ProjectionBinding::View,
            draw_function: Box::new(DrawState::<LayerItem, DrawHeatmapComposite>::new()),
            index: layer.index,
            generate_borders: false,
            style_layer: layer.id.clone(),
            tile: Tile {
                coords: any_shape.coords(),
            },
            source_shape: any_shape.clone(),
        });
    }
    Ok(())
}
