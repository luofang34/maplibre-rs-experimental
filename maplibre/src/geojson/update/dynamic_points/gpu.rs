//! GPU feature-metadata evaluation, bucket population, and per-frame visible-tile refresh.

pub(super) mod eval;

use eval::{evaluate_circle_metadata_into, fallback_dynamic_circle_from_vertices};

use super::{
    flush_deferred_geojson_bytes, seed_from_inline, source_supports_dynamic_points,
    DynamicPointGeoJson,
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
        layer::{LayerPaint, StyleLayer},
        source::{fresh_generation, Source},
        Style,
    },
    tcs::world::World,
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

    if eligible.is_empty() && world.resources.get::<DynamicPointGeoJson>().is_none() {
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
    {
        let state = world.resources.get_or_init_mut::<DynamicPointGeoJson>();
        state
            .sources
            .retain(|name, _| eligible.iter().any(|e| e == name));
        state.circle_layers = circle_layers;
        state.last_style_revision = Some(current_rev);
    }
    for name in eligible {
        let already = world
            .resources
            .get::<DynamicPointGeoJson>()
            .is_some_and(|s| s.sources.contains_key(&name));
        if !already {
            seed_from_inline(style, world, &name);
        }
    }
}

/// Builds `2 * vertices` `[fill_rgba8, dyn_radius_u32]` records for a newly uploaded bucket.
/// Returns `Some(seq)` when populated directly from `DynamicPointGeoJson`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn populate_dynamic_circle_bucket(
    dyn_points: Option<&mut DynamicPointGeoJson>,
    style_layer: &StyleLayer,
    bucket: &AvailableVectorLayerBucket,
    fallback_vertices: Option<&[crate::render::ShaderVertex]>,
    active_colors: &[[f32; 4]],
    fallback_color: [f32; 4],
    overscaled_zoom: u8,
    out: &mut Vec<FillShaderFeatureMetadata>,
) -> Option<u64> {
    let coords = bucket.coords;
    let vertices = if bucket.buffer.buffer.vertices.is_empty() {
        fallback_vertices.unwrap_or(&[])
    } else {
        bucket.buffer.buffer.vertices.as_slice()
    };
    let vertex_count = vertices.len();
    out.clear();

    if let (Some(state), Some(source_name), Some(LayerPaint::Circle(circle_paint))) = (
        dyn_points,
        style_layer.source.as_deref(),
        style_layer.paint.as_ref(),
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
            let layer_id = style_layer.layer_id();
            if !hits.is_empty()
                && (vertex_count == hits.len() * 4 || vertex_count == hits.len() * 16)
            {
                let verts_per_circle = vertex_count / hits.len();
                let tile_zoom = f64::from(overscaled_zoom.max(u8::from(coords.z)));
                evaluate_circle_metadata_into(
                    out,
                    circle_paint,
                    &src_state.properties,
                    &src_state.fast_columns,
                    hits,
                    verts_per_circle,
                    tile_zoom,
                    (seq, layer_id, &mut src_state.eval_cache),
                );
                return Some(seq);
            }
        }
    }

    fallback_dynamic_circle_from_vertices(
        out,
        &bucket.feature_indices,
        active_colors,
        fallback_color,
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
        .index()
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
}

/// Updates stale visible `CircleDynamicRadiusShader` allocations in-place, self-healing via tile
/// refresh if any resident tile's vertex count diverges from the stationary point index.
pub(crate) fn refresh_dynamic_point_layers(
    world: &mut World,
    style: &mut Style,
    queue: &UploadQueue,
    painted_tiles: &[WorldTileCoords],
) {
    if world
        .resources
        .get::<DynamicPointGeoJson>()
        .is_none_or(|s| s.sources.is_empty())
    {
        return;
    }
    let mut mismatch_sources: Vec<String> = Vec::new();
    {
        let tiles = &world.tiles;
        let Some((Initialized(buffer_pool), dyn_points)) = world
            .resources
            .query_mut::<(&mut Eventually<VectorBufferPool>, &mut DynamicPointGeoJson)>()
        else {
            return;
        };
        let DynamicPointGeoJson {
            sources,
            circle_layers,
            gpu_scratch,
            ..
        } = dyn_points;

        for (layer_idx, source_name) in circle_layers.iter() {
            let Some(style_layer) = style.layers.get(*layer_idx) else {
                continue;
            };
            if style_layer.is_hidden() {
                continue;
            }
            let Some(LayerPaint::Circle(circle_paint)) = style_layer.paint.as_ref() else {
                continue;
            };
            let Some(src_state) = sources.get_mut(source_name.as_str()) else {
                continue;
            };
            let seq = src_state.seq;
            let style_layer_id = style_layer.layer_id();

            for &coords in painted_tiles {
                let Some(entry) = buffer_pool.index().get_layers(coords).and_then(|layers| {
                    layers.iter().find(|e| {
                        e.style_layer_id == style_layer_id && e.is_dynamic_circle_metadata()
                    })
                }) else {
                    continue;
                };
                let alloc_id = entry.allocation_id();
                if src_state.applied_seq.get(&(coords, style_layer_id)) == Some(&(seq, alloc_id)) {
                    continue;
                }
                let allocated_bytes = entry.feature_metadata_buffer_range().end
                    - entry.feature_metadata_buffer_range().start;
                let allocated_records =
                    allocated_bytes as usize / std::mem::size_of::<FillShaderFeatureMetadata>();

                let hits: &[u32] = src_state
                    .tile_hits
                    .entry(coords)
                    .or_insert_with(|| super::compute_tile_hits(&src_state.world_points, coords));
                if hits.is_empty() && allocated_records == 0 {
                    src_state
                        .applied_seq
                        .insert((coords, style_layer_id), (seq, alloc_id));
                    continue;
                }
                let verts = allocated_records / 2;
                if !allocated_records.is_multiple_of(2)
                    || hits.is_empty()
                    || !(verts == hits.len() * 4 || verts == hits.len() * 16)
                {
                    if tile_retry::is_waiting_or_pending(tiles, coords, RequestKind::Vector)
                        || tiles
                            .query::<&crate::vector::content::LayerReplacements>(coords)
                            .is_some_and(|pending| !pending.vector.is_empty())
                    {
                        continue;
                    }
                    if !mismatch_sources.iter().any(|s| s == source_name) {
                        mismatch_sources.push(source_name.to_owned());
                    }
                    break;
                }

                let verts_per_circle = verts / hits.len();
                let overscaled_z = tiles
                    .query::<&VectorLayerBucketComponent>(coords)
                    .map_or(u8::from(coords.z), |b| {
                        b.overscaled_zoom.max(u8::from(coords.z))
                    });
                evaluate_circle_metadata_into(
                    gpu_scratch,
                    circle_paint,
                    &src_state.properties,
                    &src_state.fast_columns,
                    hits,
                    verts_per_circle,
                    f64::from(overscaled_z),
                    (seq, style_layer_id, &mut src_state.eval_cache),
                );
                buffer_pool.update_feature_metadata(queue, entry, gpu_scratch);
                if style.terrain.is_some() {
                    buffer_pool.bump_layer_paint_revision(coords, &style_layer.id);
                }
                src_state
                    .applied_seq
                    .insert((coords, style_layer_id), (seq, alloc_id));
            }
        }
    }

    if !mismatch_sources.is_empty() {
        flush_deferred_geojson_bytes(style, world);
        if let Some(state) = world.resources.get_mut::<DynamicPointGeoJson>() {
            for name in &mismatch_sources {
                state.sources.remove(name);
            }
        }
        let all_tiles: Vec<WorldTileCoords> =
            world.tiles.pyramid().coords_coarse_to_fine().collect();
        for name in &mismatch_sources {
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
}
