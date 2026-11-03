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
        memory_budget::{FrameUploads, UploadKind},
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
        renderer:
            Renderer {
                device,
                queue,
                settings,
                ..
            },
        ..
    }: &mut MapContext,
) -> SystemResult {
    if settings.symbol_pools.is_empty()
        || crate::render::eye_covering::EyeInFrame::reuses_content(world)
    {
        return Ok(());
    }
    let visible = super::covering::upload_tiles(world);
    world.resources.get_or_init_mut::<FrameUploads>();
    let Some((Initialized(symbol_buffer_pool), textures, Initialized(pipeline), budget)) =
        world.resources.query_mut::<(
            &mut Eventually<SymbolBufferPool>,
            &mut SymbolTextures,
            &Eventually<SymbolPipeline>,
            &mut FrameUploads,
        )>()
    else {
        return Err(SystemError::Dependencies);
    };

    textures.retain(&world.tiles);
    let zoom = view_state.style_zoom().level();

    let deferred = upload_symbol_layer(
        symbol_buffer_pool,
        textures,
        &TextureContext {
            device,
            queue,
            pipeline: &pipeline.combined,
        },
        &mut world.tiles,
        (style, &visible, zoom),
        budget,
    );
    let bound = textures.take_bound();
    if deferred {
        crate::render::frame_signals::keep_animating(world);
    }
    for coords in bound {
        crate::render::frame_signals::resource_ready(
            world,
            crate::render::frame_signals::ResourceReady::SymbolAtlas { coords },
        );
    }

    Ok(())
}

/// Uploads the labels of `visible` tiles that the frame's budget admits; `true` when some wait
/// for a later frame.
fn upload_symbol_layer(
    symbol_buffer_pool: &mut SymbolBufferPool,
    textures: &mut SymbolTextures,
    gpu: &TextureContext<'_>,
    tiles: &mut Tiles,
    (style, visible, zoom): (&Style, &[crate::coords::WorldTileCoords], f32),
    budget: &mut FrameUploads,
) -> bool {
    for &coords in visible {
        crate::vector::content::ensure(tiles, coords);
        let Some((layers, pending)) = tiles.query_mut::<(
            &mut SymbolLayersDataComponent,
            &mut crate::vector::content::LayerReplacements,
        )>(coords) else {
            continue;
        };
        let loaded: HashSet<String> = symbol_buffer_pool
            .get_loaded_style_layers_at(coords)
            .unwrap_or_default()
            .into_iter()
            .map(str::to_owned)
            .collect();
        for style_layer in style
            .layers
            .iter()
            .filter(|layer| layer.is_visible_at(f64::from(zoom)))
        {
            if let Some(committed) = layers
                .layers
                .iter()
                .find(|layer| layer.style_layer_id == style_layer.id)
            {
                prepare_atlas(textures, gpu, committed, style_layer, zoom);
            }
            let replacement = pending
                .symbols
                .iter()
                .position(|layer| layer.style_layer_id == style_layer.id);
            let upload = replacement
                .map(|index| &pending.symbols[index])
                .or_else(|| pending_layer_data(&layers.layers, &loaded, style_layer));
            let layer = upload.or_else(|| {
                layers
                    .layers
                    .iter()
                    .find(|layer| layer.style_layer_id == style_layer.id)
            });
            let Some(layer) = layer else {
                continue;
            };
            if upload.is_some() {
                let buffer = &layer.buffer;
                let size = buffer.buffer.vertices.len()
                    * (size_of::<crate::render::shaders::ShaderSymbolVertex>()
                        + size_of::<SDFShaderFeatureMetadata>())
                    + buffer.buffer.indices.len() * size_of::<u32>();
                if !budget.take(UploadKind::Geometry, size) {
                    return true;
                }
                if !upload_geometry(symbol_buffer_pool, gpu, coords, style_layer, buffer) {
                    continue;
                }
            }
            prepare_atlas(textures, gpu, layer, style_layer, zoom);
            if let Some(index) = replacement {
                crate::vector::content::commit_symbols(layers, pending.symbols.remove(index));
            }
        }
    }
    false
}

fn prepare_atlas(
    textures: &mut SymbolTextures,
    gpu: &TextureContext<'_>,
    layer: &SymbolLayerData,
    style: &StyleLayer,
    zoom: f32,
) {
    if let (Some(LayerPaint::Symbol(paint)), Some(atlas)) = (&style.paint, &layer.atlas) {
        textures.prepare(
            gpu,
            (layer.coords, style.id.clone()),
            atlas,
            paint,
            f64::from(zoom),
        );
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
        crate::render::shaders::ShaderSymbolVertex,
        u32,
    >,
) -> bool {
    // Collision placement supplies elevation before a new label can become visible.
    let feature_metadata = vec![SDFShaderFeatureMetadata::default(); buffer.buffer.vertices.len()];

    if buffer.buffer.indices.is_empty() {
        symbol_buffer_pool.remove_layer(coords, &style_layer.id);
        return true;
    }

    tracing::debug!(%coords, "allocating symbol geometry");
    if let Err(error) = symbol_buffer_pool.replace_layer_geometry(
        gpu.queue,
        coords,
        style_layer.clone(),
        buffer,
        ShaderLayerMetadata::new(style_layer.index as f32, 0.0, [0.0; 2]),
        &feature_metadata,
    ) {
        tracing::error!(%coords, %error, "tile geometry upload failed");
        return false;
    }
    true
}

#[cfg(all(test, feature = "headless", not(target_arch = "wasm32")))]
mod retry;
