//! GPU feature-metadata evaluation, bucket population, and per-frame visible-tile refresh.

pub(super) mod eval;

use eval::{
    evaluate_circle_metadata_into, fallback_dynamic_circle_from_vertices,
    populate_and_check_unchanged, CircleEvalInput,
};

use super::{
    flush_deferred_geojson_bytes, seed_from_inline, source_supports_dynamic_points,
    DynamicPointGeoJson, DynamicPointSource,
};
use crate::{
    coords::WorldTileCoords,
    io::tile_retry::{self, RequestKind},
    render::{
        eventually::{Eventually, Eventually::Initialized},
        shaders::FillShaderFeatureMetadata,
        upload_queue::UploadQueue,
    },
    style::{
        circle::CirclePaint,
        layer::{LayerPaint, StyleLayer},
        source::{fresh_generation, Source},
        Style,
    },
    tcs::{tiles::Tiles, world::World},
    vector::{AvailableVectorLayerBucket, VectorBufferPool, VectorLayerBucketComponent},
};

/// Synchronizes tracked dynamic-point GeoJSON sources when `style.revision()` changes (`O(1)` when
/// revision is unchanged).
pub(crate) fn ensure_seeded_for_style(world: &mut World, style: &Style) {
    let current_rev = style.revision();
    if world
        .resources
        .get::<DynamicPointGeoJson>()
        .is_some_and(|s| s.last_style_revision == Some(current_rev))
    {
        return;
    }
    let eligible: Vec<String> = style
        .sources
        .keys()
        .filter(|name| source_supports_dynamic_points(style, name))
        .cloned()
        .collect();

    if eligible.is_empty() {
        let state = world.resources.get_or_init_mut::<DynamicPointGeoJson>();
        state.sources.clear();
        state.circle_layers.clear();
        state.last_style_revision = Some(current_rev);
        return;
    }
    let circle_layers: Vec<(usize, String)> = style
        .layers
        .iter()
        .enumerate()
        .filter_map(|(idx, layer)| {
            if layer.is_hidden() || !matches!(layer.paint.as_ref(), Some(LayerPaint::Circle(_))) {
                return None;
            }
            let src = layer.source.as_deref()?;
            eligible
                .iter()
                .any(|e| e == src)
                .then(|| (idx, src.to_owned()))
        })
        .collect();
    let state = world.resources.get_or_init_mut::<DynamicPointGeoJson>();
    state
        .sources
        .retain(|name, _| eligible.iter().any(|e| e == name));
    state.circle_layers = circle_layers;
    state.last_style_revision = Some(current_rev);
    for name in eligible {
        if world
            .resources
            .get::<DynamicPointGeoJson>()
            .is_some_and(|s| !s.sources.contains_key(&name))
        {
            seed_from_inline(style, world, &name);
        }
    }
}

/// Inputs needed to populate dynamic-point circle GPU metadata for a newly uploaded bucket.
pub(crate) struct DynamicCircleBucketInput<'a> {
    pub(crate) dyn_points: Option<&'a mut DynamicPointGeoJson>,
    pub(crate) style_layer: &'a StyleLayer,
    pub(crate) bucket: &'a AvailableVectorLayerBucket,
    pub(crate) fallback_vertices: Option<&'a [crate::render::ShaderVertex]>,
    pub(crate) active_colors: &'a [[f32; 4]],
    pub(crate) fallback_color: [f32; 4],
    pub(crate) overscaled_zoom: u8,
}

/// Builds `2 * vertices` `[fill_rgba8, dyn_radius_u32]` records for a newly uploaded bucket.
/// Returns `Some(seq)` when populated directly from `DynamicPointGeoJson`.
pub(crate) fn populate_dynamic_circle_bucket(
    input: DynamicCircleBucketInput<'_>,
    out: &mut Vec<FillShaderFeatureMetadata>,
) -> Option<u64> {
    let coords = input.bucket.coords;
    let vertices = if input.bucket.buffer.vertices.is_empty() {
        input.fallback_vertices.unwrap_or(&[])
    } else {
        input.bucket.buffer.vertices.as_slice()
    };
    let vertex_count = vertices.len();
    out.clear();

    if let (Some(state), Some(source_name), Some(LayerPaint::Circle(circle_paint))) = (
        input.dyn_points,
        input.style_layer.source.as_deref(),
        input.style_layer.paint.as_ref(),
    ) {
        if let Some(src_state) = state.sources.get_mut(source_name) {
            let seq = src_state.seq;
            if vertex_count == 0 {
                return Some(seq);
            }
            let hits: &[u32] = src_state
                .tile_hits
                .entry(coords)
                .or_insert_with(|| super::compute_tile_hits(&src_state.world_points, coords));
            let layer_id = input.style_layer.layer_id();
            if !hits.is_empty()
                && (vertex_count == hits.len() * 4 || vertex_count == hits.len() * 16)
            {
                let verts_per_circle = vertex_count / hits.len();
                let tile_zoom = f64::from(input.overscaled_zoom.max(u8::from(coords.z)));
                evaluate_circle_metadata_into(
                    out,
                    &mut src_state.eval_cache,
                    CircleEvalInput {
                        circle_paint,
                        all_props: &src_state.properties,
                        fast_columns: &src_state.fast_columns,
                        hits,
                        verts_per_circle,
                        tile_zoom,
                        seq,
                        style_layer_id: layer_id,
                    },
                );
                return Some(seq);
            }
        }
    }

    fallback_dynamic_circle_from_vertices(
        out,
        &input.bucket.feature_indices,
        input.active_colors,
        input.fallback_color,
        vertices,
    );
    None
}

/// Records `(seq, allocation_id)` after `upload_bucket` commits a dynamic circle allocation.
pub(crate) fn record_uploaded_dynamic_circle(
    dyn_points: Option<&mut DynamicPointGeoJson>,
    buffer_pool: &VectorBufferPool,
    coords: WorldTileCoords,
    style_layer: &StyleLayer,
    seq: Option<u64>,
) {
    let (Some(state), Some(source_name), Some(seq)) =
        (dyn_points, style_layer.source.as_deref(), seq)
    else {
        return;
    };
    let Some(src_state) = state.sources.get_mut(source_name) else {
        return;
    };
    let style_layer_id = style_layer.layer_id();
    let Some(alloc_id) = buffer_pool
        .as_ref()
        .get_layers(coords)
        .and_then(|layers| {
            layers
                .iter()
                .rev()
                .find(|e| e.style_layer_id == style_layer_id)
        })
        .map(|e| e.allocation_id())
    else {
        return;
    };
    src_state
        .applied_seq
        .insert((coords, style_layer_id), (seq, alloc_id));
    src_state.clean_frame = None;
}

fn resolve_tile_layer_slot(
    src_state: &mut DynamicPointSource,
    buffer_pool: &VectorBufferPool,
    tiles: &crate::tcs::tiles::Tiles,
    coords: WorldTileCoords,
    style_layer_id: crate::style::StyleLayerId,
) -> Option<(usize, u8, u64, usize)> {
    if let Some(&cached) = src_state.slot_cache.get(&(coords, style_layer_id)) {
        return cached;
    }
    let resolved = buffer_pool
        .as_ref()
        .get_layers(coords)
        .and_then(|layers| {
            layers
                .iter()
                .enumerate()
                .find(|(_, e)| e.style_layer_id == style_layer_id && e.is_dynamic_circle_metadata())
        })
        .map(|(idx, entry)| {
            let overscaled_z = tiles
                .query::<&VectorLayerBucketComponent>(coords)
                .map_or(u8::from(coords.z), |b| {
                    b.overscaled_zoom.max(u8::from(coords.z))
                });
            let allocated_records = (entry.feature_metadata_buffer_range().end
                - entry.feature_metadata_buffer_range().start)
                as usize
                / std::mem::size_of::<FillShaderFeatureMetadata>();
            (idx, overscaled_z, entry.allocation_id(), allocated_records)
        });
    src_state
        .slot_cache
        .insert((coords, style_layer_id), resolved);
    resolved
}

fn refresh_tile_layer(
    buffer_pool: &mut VectorBufferPool,
    tiles: &Tiles,
    queue: &UploadQueue,
    coords: WorldTileCoords,
    (style_layer, circle_paint, has_terrain, has_pending_permits): (
        &StyleLayer,
        &CirclePaint,
        bool,
        bool,
    ),
    src_state: &mut DynamicPointSource,
    gpu_scratch: &mut Vec<FillShaderFeatureMetadata>,
) -> bool {
    let style_layer_id = style_layer.layer_id();
    let Some((layer_idx, overscaled_z, alloc_id, allocated_records)) =
        resolve_tile_layer_slot(src_state, buffer_pool, tiles, coords, style_layer_id)
    else {
        return true;
    };
    let seq = src_state.seq;
    let prev_applied = src_state
        .applied_seq
        .get(&(coords, style_layer_id))
        .copied();
    if prev_applied == Some((seq, alloc_id)) {
        return true;
    }
    let hits: &[u32] = src_state
        .tile_hits
        .entry(coords)
        .or_insert_with(|| super::compute_tile_hits(&src_state.world_points, coords));
    let prev_seq = prev_applied.filter(|&(_, a)| a == alloc_id).map(|(s, _)| s);
    let mark_applied = match check_tile_vertex_match(
        tiles,
        coords,
        hits,
        allocated_records,
        has_pending_permits,
    ) {
        Ok(verts_per_circle) => {
            write_evaluated_tile_layer(
                (buffer_pool, queue),
                (coords, layer_idx),
                (
                    style_layer,
                    circle_paint,
                    has_terrain,
                    overscaled_z,
                    verts_per_circle,
                ),
                (seq, prev_seq),
                (
                    &mut src_state.eval_cache,
                    &src_state.properties,
                    &src_state.fast_columns,
                    hits,
                ),
                gpu_scratch,
            );
            true
        }
        Err(true) => hits.is_empty() && allocated_records == 0,
        Err(false) => return false,
    };
    if mark_applied {
        src_state
            .applied_seq
            .insert((coords, style_layer_id), (seq, alloc_id));
    }
    true
}

fn write_evaluated_tile_layer(
    (buffer_pool, queue): (&mut VectorBufferPool, &UploadQueue),
    (coords, layer_idx): (WorldTileCoords, usize),
    (style_layer, circle_paint, has_terrain, overscaled_z, verts_per_circle): (
        &StyleLayer,
        &crate::style::circle::CirclePaint,
        bool,
        u8,
        usize,
    ),
    (seq, prev_seq_if_same_alloc): (u64, Option<u64>),
    (eval_cache, all_props, fast_columns, hits): (
        &mut super::EvalCache,
        &[crate::style::expression::FeatureProperties],
        &super::FastColumnStore,
        &[u32],
    ),
    gpu_scratch: &mut Vec<FillShaderFeatureMetadata>,
) {
    let input = CircleEvalInput {
        circle_paint,
        all_props,
        fast_columns,
        hits,
        verts_per_circle,
        tile_zoom: f64::from(overscaled_z),
        seq,
        style_layer_id: style_layer.layer_id(),
    };
    if populate_and_check_unchanged(eval_cache, &input, prev_seq_if_same_alloc) {
        return;
    }
    evaluate_circle_metadata_into(gpu_scratch, eval_cache, input);
    if let Some(entry) = buffer_pool
        .as_ref()
        .get_layers(coords)
        .and_then(|layers| layers.get(layer_idx))
    {
        buffer_pool.update_feature_metadata(queue, entry, gpu_scratch);
        if has_terrain {
            buffer_pool.bump_layer_paint_revision(coords, &style_layer.id);
        }
    }
}

fn check_tile_vertex_match(
    tiles: &crate::tcs::tiles::Tiles,
    coords: WorldTileCoords,
    hits: &[u32],
    allocated_records: usize,
    has_pending_permits: bool,
) -> Result<usize, bool> {
    if hits.is_empty() && allocated_records == 0 {
        return Err(true);
    }
    let verts = allocated_records / 2;
    if !allocated_records.is_multiple_of(2)
        || hits.is_empty()
        || !(verts == hits.len() * 4 || verts == hits.len() * 16)
    {
        let pending = has_pending_permits
            || tile_retry::is_waiting_or_pending(tiles, coords, RequestKind::Vector)
            || tiles
                .query::<&crate::vector::content::LayerReplacements>(coords)
                .is_some_and(|pending| !pending.vector.is_empty());
        return Err(pending);
    }
    Ok(verts / hits.len())
}

fn trigger_mismatched_source_refresh(
    world: &mut World,
    style: &mut Style,
    mismatch_sources: &[String],
) {
    flush_deferred_geojson_bytes(style, world);
    if let Some(state) = world.resources.get_mut::<DynamicPointGeoJson>() {
        for name in mismatch_sources {
            state.sources.remove(name);
        }
    }
    let all_tiles: Vec<WorldTileCoords> = world.tiles.coords_coarse_to_fine().collect();
    for name in mismatch_sources {
        if let Some(Source::GeoJson(src)) = style.sources.get_mut(name) {
            src.generation = fresh_generation();
        }
        if !all_tiles.is_empty() {
            tile_retry::refresh_tiles_for_sources(
                world,
                RequestKind::Vector,
                &all_tiles,
                std::slice::from_ref(name),
            );
        }
    }
    style.bump_revision();
    crate::render::frame_signals::mark_dirty(world);
}

fn refresh_source_layer_tiles(
    buffer_pool: &mut VectorBufferPool,
    tiles: &Tiles,
    queue: &UploadQueue,
    painted_tiles: &[WorldTileCoords],
    layer_cfg: (&StyleLayer, &CirclePaint, bool, bool),
    src_state: &mut DynamicPointSource,
    gpu_scratch: &mut Vec<FillShaderFeatureMetadata>,
) -> bool {
    painted_tiles.iter().all(|&coords| {
        refresh_tile_layer(
            buffer_pool,
            tiles,
            queue,
            coords,
            layer_cfg,
            src_state,
            gpu_scratch,
        )
    })
}

/// Updates stale visible `CircleDynamicRadiusShader` allocations in-place, self-healing via tile
/// refresh if any resident tile's vertex count diverges from the stationary point index.
pub(crate) fn refresh_dynamic_point_layers(
    world: &mut World,
    style: &mut Style,
    queue: &UploadQueue,
    painted_tiles: &[WorldTileCoords],
) {
    let has_pending_permits = world
        .resources
        .get::<crate::vector::request_system::PendingUploadPermits>()
        .is_some_and(|permits| !permits.is_empty());
    let mut mismatch_sources: Vec<String> = Vec::new();
    {
        let tiles = &world.tiles;
        let Some((Initialized(buffer_pool), dyn_points)) = world
            .resources
            .query_mut::<(&mut Eventually<VectorBufferPool>, &mut DynamicPointGeoJson)>()
        else {
            return;
        };
        if dyn_points.sources.is_empty() {
            return;
        }
        let has_terrain = style.terrain.is_some();
        let pool_state = (buffer_pool.revision(), buffer_pool.eviction_generation());
        for (layer_idx, source_name) in &dyn_points.circle_layers {
            let Some(style_layer) = style.layers.get(*layer_idx).filter(|l| !l.is_hidden()) else {
                continue;
            };
            let (Some(LayerPaint::Circle(circle_paint)), Some(src_state)) = (
                style_layer.paint.as_ref(),
                dyn_points.sources.get_mut(source_name.as_str()),
            ) else {
                continue;
            };
            if src_state.slot_cache_state != Some(pool_state) {
                src_state.slot_cache.clear();
                src_state.slot_cache_state = Some(pool_state);
                src_state.clean_frame = None;
            }
            if !has_pending_permits && src_state.matches_clean_frame(pool_state, painted_tiles) {
                continue;
            }
            let all_clean = refresh_source_layer_tiles(
                buffer_pool,
                tiles,
                queue,
                painted_tiles,
                (style_layer, circle_paint, has_terrain, has_pending_permits),
                src_state,
                &mut dyn_points.gpu_scratch,
            );
            if !all_clean && !mismatch_sources.iter().any(|s| s == source_name) {
                mismatch_sources.push(source_name.to_owned());
            }
            src_state.set_clean_frame(pool_state, painted_tiles, all_clean && !has_pending_permits);
        }
    }

    if !mismatch_sources.is_empty() {
        trigger_mismatched_source_refresh(world, style, &mismatch_sources);
    }
}
