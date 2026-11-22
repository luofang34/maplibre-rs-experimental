//! Uploads data to the GPU which is needed for rendering.

mod layer_upload;
use layer_upload::upload_symbol_layer;

use super::{
    textures::{SymbolTextures, TextureContext},
    SymbolPipeline,
};
use crate::{
    context::MapContext,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        memory_budget::FrameUploads,
        shaders::{EmptyLayerMetadata, SDFShaderFeatureMetadata},
        Renderer,
    },
    sdf::{SymbolBufferPool, SymbolLayerData},
    style::{
        layer::{LayerPaint, StyleLayer},
        Style,
    },
    tcs::system::{SystemError, SystemResult},
};

#[derive(Default)]
struct SymbolStyleIndex {
    style_rev: Option<u64>,
    by_id: rustc_hash::FxHashMap<String, usize>,
    symbol_indices: Vec<usize>,
}

impl SymbolStyleIndex {
    fn sync(&mut self, style: &Style) {
        let rev = style.revision();
        if self.style_rev == Some(rev) {
            return;
        }
        self.style_rev = Some(rev);
        self.by_id.clear();
        self.symbol_indices.clear();
        for (idx, layer) in style.layers.iter().enumerate() {
            if !layer.is_hidden() && layer.type_ == "symbol" {
                self.by_id.insert(layer.id.clone(), idx);
                self.symbol_indices.push(idx);
            }
        }
    }
}

pub(super) fn atlas_zoom_target(zoom: f32, moving: bool) -> (i32, i32, f32) {
    let exact_zoom_q32 = (zoom * 32.0).round() as i32;
    let target_zoom_q32 = if moving {
        ((zoom * 8.0).round() as i32) * 4
    } else {
        exact_zoom_q32
    };
    let atlas_zoom = if moving {
        target_zoom_q32 as f32 / 32.0
    } else {
        zoom
    };
    (exact_zoom_q32, target_zoom_q32, atlas_zoom)
}

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
    if !super::style_has_symbol_layers(style) {
        if let Some(textures) = world.resources.get_mut::<SymbolTextures>() {
            textures.clear();
        }
        if super::symbol_pool_empty(world) {
            return Ok(());
        }
    }
    let visible = super::covering::upload_tiles(world);
    let arrival_epoch = crate::vector::arrival_epoch(world);
    world.resources.get_or_init_mut::<FrameUploads>();
    world
        .resources
        .get_or_init_mut::<SymbolStyleIndex>()
        .sync(style);
    let Some((
        Initialized(symbol_buffer_pool),
        textures,
        Initialized(pipeline),
        budget,
        symbol_style,
    )) = world.resources.query_mut::<(
        &mut Eventually<SymbolBufferPool>,
        &mut SymbolTextures,
        &Eventually<SymbolPipeline>,
        &mut FrameUploads,
        &SymbolStyleIndex,
    )>()
    else {
        return Err(SystemError::Dependencies);
    };

    textures.retain_if_needed(&world.tiles, style, arrival_epoch);
    let zoom = view_state.style_zoom().level();

    let (deferred, needs_refetch, needs_atlas_settle) = upload_symbol_layer(
        symbol_buffer_pool,
        textures,
        &TextureContext {
            device,
            queue,
            atlas_layout: &pipeline.atlas_layout,
        },
        &mut world.tiles,
        (style, symbol_style, &visible, zoom),
        budget,
    );
    let bound = textures.take_bound();
    if deferred || needs_atlas_settle {
        crate::render::frame_signals::keep_animating(world);
    }
    if needs_refetch {
        crate::render::frame_signals::mark_dirty(world);
    }
    for coords in bound {
        crate::render::frame_signals::resource_ready(
            world,
            crate::render::frame_signals::ResourceReady::SymbolAtlas { coords },
        );
    }

    Ok(())
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
            (layer.coords, style.layer_id()),
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
    loaded_style_layers: &rustc_hash::FxHashSet<crate::style::StyleLayerId>,
    style_layer: &StyleLayer,
) -> Option<&'a SymbolLayerData> {
    if loaded_style_layers.contains(&style_layer.layer_id()) {
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
    visible_set: &rustc_hash::FxHashSet<crate::coords::WorldTileCoords>,
) -> bool {
    if buffer.buffer.indices.is_empty() {
        symbol_buffer_pool.remove_layer(coords, &style_layer.id);
        return true;
    }
    // Collision placement supplies elevation before a new label can become visible.
    tracing::debug!(%coords, "allocating symbol geometry");
    if let Err(error) = symbol_buffer_pool.replace_layer_geometry_uniform_unless(
        gpu.queue,
        coords,
        crate::vector::UniformLayerUpload {
            style_layer: style_layer.clone().into(),
            geometry: buffer,
            layer_metadata: EmptyLayerMetadata([0; 0]),
            uniform_feature: SDFShaderFeatureMetadata::default(),
            records: buffer.buffer.vertices.len(),
        },
        |candidate| visible_set.contains(&candidate),
    ) {
        tracing::error!(%coords, %error, "tile geometry upload failed");
        return false;
    }
    true
}

#[cfg(all(test, feature = "headless", not(target_arch = "wasm32")))]
mod retry;
