//! Uploads data to the GPU which is needed for rendering.

use std::collections::HashSet;

use crate::{
    context::MapContext,
    coords::ViewRegion,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::view_region_for_projection,
        shaders::{SDFShaderFeatureMetadata, ShaderLayerMetadata},
        tile_view_pattern::DEFAULT_TILE_SIZE,
        view_state::ViewStatePadding,
        Renderer,
    },
    sdf::{SymbolBufferPool, SymbolLayerData, SymbolLayersDataComponent},
    style::{
        layer::{LayerPaint, StyleLayer},
        Style,
    },
    tcs::{
        system::{SystemError, SystemResult},
        tiles::Tiles,
    },
};

pub fn upload_system(
    MapContext {
        world,
        style,
        view_state,
        renderer: Renderer { queue, .. },
        ..
    }: &mut MapContext,
) -> SystemResult {
    let view_region = view_region_for_projection(
        style,
        view_state,
        world,
        view_state.zoom().zoom_level(DEFAULT_TILE_SIZE),
        ViewStatePadding::Loose,
    )
    .map_err(|error| {
        tracing::error!(%error, "unable to select symbol upload tiles");
        SystemError::Setup
    })?;

    let Some(Initialized(symbol_buffer_pool)) = world
        .resources
        .query_mut::<&mut Eventually<SymbolBufferPool>>()
    else {
        return Err(SystemError::Dependencies);
    };

    let zoom = view_state.zoom().level();

    if let Some(view_region) = &view_region {
        upload_symbol_layer(
            symbol_buffer_pool,
            queue,
            &mut world.tiles,
            style,
            view_region,
            zoom,
        );
    }

    Ok(())
}

// TODO cleanup, duplicated
fn upload_symbol_layer(
    symbol_buffer_pool: &mut SymbolBufferPool,
    queue: &wgpu::Queue,
    tiles: &mut Tiles,
    style: &Style,
    view_region: &ViewRegion,
    zoom: f32,
) {
    // Upload all tessellated layers which are in view
    for coords in view_region.iter() {
        let Some(vector_layers) = tiles.query_mut::<&SymbolLayersDataComponent>(coords) else {
            continue;
        };

        let loaded_layers: HashSet<String> = symbol_buffer_pool
            .get_loaded_style_layers_at(coords)
            .unwrap_or_default()
            .into_iter()
            .map(str::to_string)
            .collect();

        for style_layer in &style.layers {
            let Some(SymbolLayerData {
                coords,
                new_buffer: buffer,
                ..
            }) = pending_layer_data(&vector_layers.layers, &loaded_layers, style_layer)
            else {
                continue;
            };

            // One opacity entry per vertex, visible until collision detection hides a label;
            // the features of a layout that does not attribute quads to labels cover no
            // vertices, so the vertex count is the only reliable size.
            let feature_metadata =
                vec![SDFShaderFeatureMetadata { opacity: 1.0 }; buffer.buffer.vertices.len()];

            // FIXME avoid uploading empty indices
            if buffer.buffer.indices.is_empty() {
                continue;
            }

            // Extract text-size from style (default 16.0 per MapLibre GL JS spec)
            let text_size = match &style_layer.paint {
                Some(LayerPaint::Symbol(paint)) => paint
                    .text_size
                    .as_ref()
                    .and_then(|s| s.evaluate_at_zoom(f64::from(zoom)))
                    .unwrap_or(16.0),
                _ => 16.0,
            };

            log::debug!("Allocating geometry at {coords}");
            symbol_buffer_pool.allocate_layer_geometry(
                queue,
                *coords,
                style_layer.clone(),
                buffer,
                // The line width slot carries the text size for the SDF pipeline.
                ShaderLayerMetadata::new(style_layer.index as f32, text_size, [0.0; 2]),
                &feature_metadata,
            );
        }
    }
}

/// The tessellated data of a style layer that is not in the pool yet. Style layers sharing a
/// source layer each have their own data, so the match is on the style layer id: matching on
/// the source layer would upload one layer's geometry under every sibling's name.
fn pending_layer_data<'a>(
    layers: &'a [SymbolLayerData],
    loaded_style_layers: &HashSet<String>,
    style_layer: &StyleLayer,
) -> Option<&'a SymbolLayerData> {
    if loaded_style_layers.contains(&style_layer.id) {
        return None;
    }
    layers
        .iter()
        .find(|layer| layer.style_layer_id == style_layer.id)
}

#[cfg(test)]
mod tests;
