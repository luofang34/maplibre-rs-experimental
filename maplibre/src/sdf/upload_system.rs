//! Uploads data to the GPU which is needed for rendering.

use super::{
    textures::{SymbolTextures, TextureContext},
    SymbolPipeline,
};
use std::collections::HashSet;

use crate::{
    context::MapContext,
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
        renderer: Renderer { device, queue, .. },
        ..
    }: &mut MapContext,
) -> SystemResult {
    if crate::render::eye_covering::EyeInFrame::reuses_content(world) {
        return Ok(());
    }
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

    let mut visible = std::collections::HashSet::new();
    if let Some(region) = &view_region {
        visible.extend(region.iter());
    }
    if let Some(Initialized(pattern)) = world
        .resources
        .get::<Eventually<crate::render::tile_view_pattern::WgpuTileViewPattern>>()
    {
        for tile in pattern.iter() {
            tile.render_kind(crate::io::tile_sources::TileKind::Vector, |shape| {
                visible.insert(shape.coords());
            });
        }
    }
    let mut visible: Vec<_> = visible.into_iter().collect();
    visible.sort_by_key(|coords| (u8::from(coords.z), coords.y, coords.x));
    let Some((Initialized(symbol_buffer_pool), textures, Initialized(pipeline))) =
        world.resources.query_mut::<(
            &mut Eventually<SymbolBufferPool>,
            &mut SymbolTextures,
            &Eventually<SymbolPipeline>,
        )>()
    else {
        return Err(SystemError::Dependencies);
    };

    textures.retain(&world.tiles);
    let zoom = view_state.zoom().level();

    {
        upload_symbol_layer(
            symbol_buffer_pool,
            textures,
            &TextureContext {
                device,
                queue,
                pipeline,
            },
            &mut world.tiles,
            style,
            &visible,
            zoom,
        );
    }

    Ok(())
}

// TODO cleanup, duplicated
fn upload_symbol_layer(
    symbol_buffer_pool: &mut SymbolBufferPool,
    textures: &mut SymbolTextures,
    gpu: &TextureContext<'_>,
    tiles: &mut Tiles,
    style: &Style,
    visible: &[crate::coords::WorldTileCoords],
    zoom: f32,
) {
    // Upload all tessellated layers which are in view
    for &coords in visible {
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
            if let Some(LayerPaint::Symbol(paint)) = &style_layer.paint {
                if let Some(layer) = vector_layers
                    .layers
                    .iter()
                    .find(|layer| layer.style_layer_id == style_layer.id)
                {
                    if let Some(atlas) = &layer.atlas {
                        textures.prepare(
                            gpu,
                            (coords, style_layer.id.clone()),
                            atlas,
                            paint,
                            f64::from(zoom),
                        );
                    }
                }
            }
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
            let feature_metadata = vec![
                SDFShaderFeatureMetadata {
                    opacity: 1.0,
                    elevation: 0.0
                };
                buffer.buffer.vertices.len()
            ];

            // FIXME avoid uploading empty indices
            if buffer.buffer.indices.is_empty() {
                continue;
            }

            log::debug!("Allocating geometry at {coords}");
            symbol_buffer_pool.allocate_layer_geometry(
                gpu.queue,
                *coords,
                style_layer.clone(),
                buffer,
                ShaderLayerMetadata::new(style_layer.index as f32, 0.0, [0.0; 2]),
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
