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

#[derive(Default)]
pub(crate) struct SdfUploadPending(pub bool);

#[derive(Clone, PartialEq, Eq)]
struct SdfUploadKey {
    arrival_epoch: u64,
    style_rev: u64,
    tiles_len: usize,
    topology_gen: u64,
    covering_tiles_version: u64,
    pool_rev: u64,
    pool_evict: u64,
    target_zoom_q32: i32,
    exact_zoom_q32: i32,
    moving: bool,
}

#[derive(Default)]
struct SdfUploadCache {
    last_key: Option<SdfUploadKey>,
    last_deferred: bool,
    last_needs_atlas_settle: bool,
    last_needs_refetch: bool,
}

impl SdfUploadCache {
    fn build_key(world: &crate::tcs::world::World, style: &Style, zoom: f32) -> SdfUploadKey {
        let moving = world
            .resources
            .get::<FrameUploads>()
            .is_some_and(FrameUploads::is_moving);
        let (exact_zoom_q32, target_zoom_q32, _) = atlas_zoom_target(zoom, moving);
        let topology_gen = match world
            .resources
            .get::<Eventually<crate::render::tile_view_pattern::WgpuTileViewPattern>>()
        {
            Some(Initialized(pattern)) => pattern.topology_generation(),
            _ => 0,
        };
        let covering_tiles_version = world
            .resources
            .get::<super::covering::SymbolCovering>()
            .map_or(0, |covering| covering.tiles_version);
        let (pool_rev, pool_evict) = match world.resources.get::<Eventually<SymbolBufferPool>>() {
            Some(Initialized(pool)) => (pool.revision(), pool.eviction_generation()),
            _ => (0, 0),
        };
        SdfUploadKey {
            arrival_epoch: crate::vector::arrival_epoch(world),
            style_rev: style.revision(),
            tiles_len: world
                .tiles
                .pyramid_of::<super::SymbolLayersDataComponent>()
                .len(),
            topology_gen,
            covering_tiles_version,
            pool_rev,
            pool_evict,
            target_zoom_q32,
            exact_zoom_q32,
            moving,
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
    if crate::render::eye_covering::EyeInFrame::reuses_content(world) {
        return Ok(());
    }
    if settings.symbol_pools.is_empty() {
        world.resources.insert(SdfUploadPending(false));
        return Ok(());
    }
    if !super::style_has_symbol_layers(style) {
        if let Some(textures) = world.resources.get_mut::<SymbolTextures>() {
            textures.clear();
        }
        if super::symbol_pool_empty(world) {
            world.resources.insert(SdfUploadPending(false));
            return Ok(());
        }
    }
    let zoom = view_state.style_zoom().level();
    let pre_key = SdfUploadCache::build_key(world, style, zoom);
    if world.resources.get::<SdfUploadCache>().is_some_and(|c| {
        !c.last_deferred
            && !c.last_needs_atlas_settle
            && !c.last_needs_refetch
            && c.last_key.as_ref() == Some(&pre_key)
    }) {
        return Ok(());
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
    let post_pool = (
        symbol_buffer_pool.revision(),
        symbol_buffer_pool.eviction_generation(),
    );
    let bound = textures.take_bound();
    world
        .resources
        .insert(SdfUploadPending(deferred || needs_atlas_settle));
    let mut post_key = pre_key;
    post_key.pool_rev = post_pool.0;
    post_key.pool_evict = post_pool.1;
    world.resources.insert(SdfUploadCache {
        last_key: Some(post_key),
        last_deferred: deferred,
        last_needs_atlas_settle: needs_atlas_settle,
        last_needs_refetch: needs_refetch,
    });
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
