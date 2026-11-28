//! Per-tile symbol geometry and atlas upload pass for [`super::upload_system`].

use super::{
    atlas_zoom_target, pending_layer_data, prepare_atlas,
    replacements::{commit_symbols, ensure, SymbolLayerReplacements},
    upload_geometry, SymbolStyleIndex,
};
use crate::{
    render::{
        memory_budget::{FrameUploads, UploadKind},
        resource::OverAlignedVertexBuffer,
        shaders::SDFShaderFeatureMetadata,
    },
    sdf::{
        textures::{SymbolTextures, TextureContext},
        SymbolBufferPool, SymbolLayersDataComponent,
    },
    style::{layer::StyleLayer, Style},
    tcs::tiles::Tiles,
};

struct SymbolLayerUploadCtx<'a> {
    symbol_buffer_pool: &'a mut SymbolBufferPool,
    textures: &'a mut SymbolTextures,
    gpu: &'a TextureContext<'a>,
    style: &'a Style,
    symbol_style: &'a SymbolStyleIndex,
    visible_set: &'a rustc_hash::FxHashSet<crate::coords::WorldTileCoords>,
    budget: &'a mut FrameUploads,
    atlas_zoom: f32,
    exact_zoom_q32: i32,
    target_zoom_q32: i32,
    moving: bool,
    stamp: (u64, u64),
    upload_started: instant::Instant,
    sdf_time_budget: Option<std::time::Duration>,
}

struct UploadProgress {
    did_gpu_work: bool,
    needs_refetch: bool,
    needs_atlas_settle: bool,
}

fn all_tiles_uploaded(
    tiles: &Tiles,
    visible: &[crate::coords::WorldTileCoords],
    stamp: (u64, u64),
    target_zoom_q32: i32,
) -> bool {
    visible.iter().all(|&coords| {
        tiles
            .query::<(&SymbolLayersDataComponent, &SymbolLayerReplacements)>(coords)
            .is_some_and(|(layers, pending)| {
                !layers.pending_assets
                    && pending.symbols.is_empty()
                    && layers.uploaded_stamp == Some(stamp)
                    && layers.atlas_zoom_q32 == Some(target_zoom_q32)
            })
    })
}

fn time_budget_spent(ctx: &SymbolLayerUploadCtx<'_>, progress: &UploadProgress) -> bool {
    ctx.sdf_time_budget
        .is_some_and(|limit| progress.did_gpu_work && ctx.upload_started.elapsed() >= limit)
}

fn update_committed_atlases(
    ctx: &mut SymbolLayerUploadCtx<'_>,
    layers: &mut SymbolLayersDataComponent,
    progress: &mut UploadProgress,
) -> bool {
    if ctx.moving && ctx.target_zoom_q32 != ctx.exact_zoom_q32 && !layers.layers.is_empty() {
        progress.needs_atlas_settle = true;
    }
    if layers.atlas_zoom_q32 == Some(ctx.target_zoom_q32) {
        return true;
    }
    if time_budget_spent(ctx, progress) {
        return false;
    }
    for committed in &layers.layers {
        if let Some(style_layer) = ctx
            .symbol_style
            .layer(ctx.style, committed.style_layer_id.as_str())
        {
            prepare_atlas(
                ctx.textures,
                ctx.gpu,
                committed,
                style_layer,
                ctx.atlas_zoom,
            );
            progress.did_gpu_work = true;
        }
    }
    layers.atlas_zoom_q32 = Some(ctx.target_zoom_q32);
    true
}

fn upload_single_symbol_layer(
    ctx: &mut SymbolLayerUploadCtx<'_>,
    coords: crate::coords::WorldTileCoords,
    style_layer: &StyleLayer,
    loaded: &rustc_hash::FxHashSet<crate::style::StyleLayerId>,
    (layers, pending, progress, tile_incomplete): (
        &mut SymbolLayersDataComponent,
        &mut SymbolLayerReplacements,
        &mut UploadProgress,
        &mut bool,
    ),
) -> bool {
    let replacement = pending
        .symbols
        .iter()
        .position(|layer| layer.style_layer_id == style_layer.id);
    let upload = replacement
        .and_then(|index| pending.symbols.get(index))
        .or_else(|| pending_layer_data(&layers.layers, loaded, style_layer));
    let Some(layer) = upload.or_else(|| {
        layers
            .layers
            .iter()
            .find(|layer| layer.style_layer_id == style_layer.id)
    }) else {
        return true;
    };
    let did_upload = if upload.is_some() {
        let buffer = &layer.buffer;
        if replacement.is_none() && buffer.usable_indices > 0 && buffer.vertices.is_empty() {
            progress.needs_refetch = true;
            *tile_incomplete = true;
            return true;
        }
        let size = symbol_upload_bytes(buffer);
        if !ctx.budget.can_take(UploadKind::Geometry, size) || time_budget_spent(ctx, progress) {
            if !ctx.budget.can_take(UploadKind::Geometry, size) && replacement.is_some() {
                prepare_committed_atlas(ctx, layers, style_layer);
            }
            return false;
        }
        if !upload_geometry(
            ctx.symbol_buffer_pool,
            ctx.gpu,
            coords,
            style_layer,
            buffer,
            ctx.visible_set,
        ) {
            *tile_incomplete = true;
            if replacement.is_some() {
                prepare_committed_atlas(ctx, layers, style_layer);
            }
            return true;
        }
        ctx.budget.take(UploadKind::Geometry, size);
        true
    } else {
        false
    };
    prepare_atlas(ctx.textures, ctx.gpu, layer, style_layer, ctx.atlas_zoom);
    progress.did_gpu_work |=
        did_upload || (layer.atlas.is_some() && layers.atlas_zoom_q32 != Some(ctx.target_zoom_q32));
    commit_symbol_upload(layers, pending, replacement, did_upload, style_layer);
    true
}

fn symbol_upload_bytes(
    buffer: &OverAlignedVertexBuffer<crate::render::shaders::ShaderSymbolVertex, u32>,
) -> usize {
    let index_stride = if buffer.vertices.len() <= 65_536 {
        size_of::<u16>()
    } else {
        size_of::<u32>()
    };
    buffer.vertices.len()
        * (size_of::<crate::render::shaders::ShaderSymbolVertex>()
            + size_of::<SDFShaderFeatureMetadata>())
        + buffer.indices.len() * index_stride
}

fn prepare_committed_atlas(
    ctx: &mut SymbolLayerUploadCtx<'_>,
    layers: &SymbolLayersDataComponent,
    style_layer: &StyleLayer,
) {
    if let Some(committed) = layers
        .layers
        .iter()
        .find(|item| item.style_layer_id == style_layer.id)
    {
        prepare_atlas(
            ctx.textures,
            ctx.gpu,
            committed,
            style_layer,
            ctx.atlas_zoom,
        );
    }
}

fn commit_symbol_upload(
    layers: &mut SymbolLayersDataComponent,
    pending: &mut SymbolLayerReplacements,
    replacement: Option<usize>,
    did_upload: bool,
    style_layer: &StyleLayer,
) {
    if let Some(index) = replacement {
        let mut committed = pending.symbols.remove(index);
        committed.buffer.release_uploaded_symbol_vertices();
        commit_symbols(layers, committed);
    } else if did_upload {
        if let Some(committed) = layers
            .layers
            .iter_mut()
            .find(|item| item.style_layer_id == style_layer.id)
        {
            committed.buffer.release_uploaded_symbol_vertices();
        }
    }
}

/// Uploads the labels of `visible` tiles that the frame's budget admits; returns
/// `(deferred, needs_refetch, needs_atlas_settle)`.
pub(crate) fn upload_symbol_layer(
    symbol_buffer_pool: &mut SymbolBufferPool,
    textures: &mut SymbolTextures,
    gpu: &TextureContext<'_>,
    tiles: &mut Tiles,
    (style, symbol_style, visible, zoom): (
        &Style,
        &SymbolStyleIndex,
        &[crate::coords::WorldTileCoords],
        f32,
    ),
    budget: &mut FrameUploads,
) -> (bool, bool, bool) {
    let stamp = (style.revision(), symbol_buffer_pool.eviction_generation());
    let moving = budget.is_moving();
    let (exact_zoom_q32, target_zoom_q32, atlas_zoom) = atlas_zoom_target(zoom, moving);
    if all_tiles_uploaded(tiles, visible, stamp, target_zoom_q32) {
        let needs_settle = moving
            && target_zoom_q32 != exact_zoom_q32
            && visible.iter().any(|&coords| {
                tiles
                    .query::<&SymbolLayersDataComponent>(coords)
                    .is_some_and(|layers| !layers.layers.is_empty())
            });
        return (false, false, needs_settle);
    }
    let visible_set: rustc_hash::FxHashSet<_> = visible.iter().copied().collect();
    let sdf_time_budget = budget.sdf_upload_time_budget();
    let mut ctx = SymbolLayerUploadCtx {
        symbol_buffer_pool,
        textures,
        gpu,
        style,
        symbol_style,
        visible_set: &visible_set,
        budget,
        atlas_zoom,
        exact_zoom_q32,
        target_zoom_q32,
        moving,
        stamp,
        upload_started: instant::Instant::now(),
        sdf_time_budget,
    };
    let mut progress = UploadProgress {
        did_gpu_work: false,
        needs_refetch: false,
        needs_atlas_settle: false,
    };
    for &coords in visible {
        if !upload_tile_symbols(&mut ctx, tiles, coords, &mut progress) {
            return (true, progress.needs_refetch, progress.needs_atlas_settle);
        }
    }
    (false, progress.needs_refetch, progress.needs_atlas_settle)
}

fn upload_tile_symbols(
    ctx: &mut SymbolLayerUploadCtx<'_>,
    tiles: &mut Tiles,
    coords: crate::coords::WorldTileCoords,
    progress: &mut UploadProgress,
) -> bool {
    ensure(tiles, coords);
    let Some((layers, pending)) =
        tiles.query_mut::<(&mut SymbolLayersDataComponent, &mut SymbolLayerReplacements)>(coords)
    else {
        return true;
    };
    if !layers.pending_assets
        && pending.symbols.is_empty()
        && layers.uploaded_stamp == Some(ctx.stamp)
    {
        return update_committed_atlases(ctx, layers, progress);
    }
    if time_budget_spent(ctx, progress) {
        return false;
    }
    let mut tile_incomplete = false;
    let loaded = ctx.symbol_buffer_pool.loaded_style_layer_ids_at(coords);
    for &idx in &ctx.symbol_style.symbol_indices {
        let Some(style_layer) = ctx.style.layers.get(idx) else {
            continue;
        };
        if !upload_single_symbol_layer(
            ctx,
            coords,
            style_layer,
            &loaded,
            (layers, pending, progress, &mut tile_incomplete),
        ) {
            return false;
        }
    }
    if !tile_incomplete && !layers.pending_assets && pending.symbols.is_empty() {
        layers.uploaded_stamp = Some(ctx.stamp);
        layers.atlas_zoom_q32 = Some(ctx.target_zoom_q32);
        if ctx.moving && ctx.target_zoom_q32 != ctx.exact_zoom_q32 && !layers.layers.is_empty() {
            progress.needs_atlas_settle = true;
        }
    }
    true
}
