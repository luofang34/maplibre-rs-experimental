//! Uploads data to the GPU which is needed for rendering.

use std::collections::HashSet;

use super::{
    textures::{SymbolTextures, TextureContext},
    SymbolPipeline,
};
use crate::{
    context::MapContext,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        shaders::{SDFShaderFeatureMetadata, ShaderLayerMetadata},
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
    let visible = super::covering::upload_tiles(world);
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

fn upload_symbol_layer(
    symbol_buffer_pool: &mut SymbolBufferPool,
    textures: &mut SymbolTextures,
    gpu: &TextureContext<'_>,
    tiles: &mut Tiles,
    style: &Style,
    visible: &[crate::coords::WorldTileCoords],
    zoom: f32,
) {
    let mut bytes = crate::render::memory_budget::UploadBudget::new(8 << 20);
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

        for style_layer in style
            .layers
            .iter()
            .filter(|layer| layer.is_visible_at(f64::from(zoom)))
        {
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

            let size = buffer.buffer.vertices.len()
                * (size_of::<crate::render::shaders::ShaderSymbolVertexNew>()
                    + size_of::<SDFShaderFeatureMetadata>())
                + buffer.buffer.indices.len() * size_of::<u32>();
            if !bytes.take(size) {
                return;
            }
            upload_geometry(symbol_buffer_pool, gpu, *coords, style_layer, buffer);
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

fn upload_geometry(
    symbol_buffer_pool: &mut SymbolBufferPool,
    gpu: &TextureContext<'_>,
    coords: crate::coords::WorldTileCoords,
    style_layer: &StyleLayer,
    buffer: &crate::vector::tessellation::OverAlignedVertexBuffer<
        crate::render::shaders::ShaderSymbolVertexNew,
        u32,
    >,
) {
    // Collision placement supplies elevation before a new label can become visible.
    let feature_metadata = vec![
        SDFShaderFeatureMetadata {
            opacity: 0.0,
            elevation: 0.0
        };
        buffer.buffer.vertices.len()
    ];

    // FIXME avoid uploading empty indices
    if buffer.buffer.indices.is_empty() {
        return;
    }

    tracing::debug!(%coords, "allocating symbol geometry");
    if let Err(error) = symbol_buffer_pool.allocate_layer_geometry(
        gpu.queue,
        coords,
        style_layer.clone(),
        buffer,
        ShaderLayerMetadata::new(style_layer.index as f32, 0.0, [0.0; 2]),
        &feature_metadata,
    ) {
        tracing::error!(%coords, %error, "tile geometry upload failed");
    }
}
