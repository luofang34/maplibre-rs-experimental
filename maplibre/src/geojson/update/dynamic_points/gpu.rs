//! GPU feature-metadata evaluation, bucket population, and per-frame visible-tile refresh.

use super::{
    flush_deferred_geojson_bytes,
    ramp::{ColorRamp, ScalarRamp},
    seed_from_inline, source_supports_dynamic_points, DynamicPointGeoJson, EvalCache,
};
use crate::{
    coords::WorldTileCoords,
    io::tile_retry::{self, RequestKind},
    render::{
        eventually::{Eventually, Eventually::Initialized},
        shaders::{pack_dynamic_circle_radii, FillShaderFeatureMetadata},
        upload_queue::UploadQueue,
    },
    style::{
        circle::CirclePaint,
        expression::FeatureProperties,
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
    {
        let state = world.resources.get_or_init_mut::<DynamicPointGeoJson>();
        state
            .sources
            .retain(|name, _| eligible.iter().any(|e| e == name));
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
            gpu_scratch,
            ..
        } = dyn_points;

        for style_layer in &style.layers {
            if style_layer.is_hidden() {
                continue;
            }
            let (Some(source_name), Some(LayerPaint::Circle(circle_paint))) =
                (style_layer.source.as_deref(), style_layer.paint.as_ref())
            else {
                continue;
            };
            let Some(src_state) = sources.get_mut(source_name) else {
                continue;
            };
            let seq = src_state.seq;
            let style_layer_id = style_layer.layer_id();

            for &coords in painted_tiles {
                let entry_info = buffer_pool
                    .index()
                    .get_layers(coords)
                    .and_then(|layers| {
                        layers.iter().find(|e| {
                            e.style_layer_id == style_layer_id && e.is_dynamic_circle_metadata()
                        })
                    })
                    .map(|e| {
                        let bytes = e.feature_metadata_buffer_range().end
                            - e.feature_metadata_buffer_range().start;
                        (
                            e.allocation_id(),
                            bytes as usize / std::mem::size_of::<FillShaderFeatureMetadata>(),
                        )
                    });
                let Some((alloc_id, allocated_records)) = entry_info else {
                    continue;
                };
                if src_state.applied_seq.get(&(coords, style_layer_id)) == Some(&(seq, alloc_id)) {
                    continue;
                }

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
                if allocated_records % 2 != 0
                    || hits.is_empty()
                    || !(verts == hits.len() * 4 || verts == hits.len() * 16)
                {
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
                    hits,
                    verts_per_circle,
                    f64::from(overscaled_z),
                    (seq, style_layer_id, &mut src_state.eval_cache),
                );
                if let Some(entry) = buffer_pool.index().get_layers(coords).and_then(|layers| {
                    layers.iter().find(|e| {
                        e.style_layer_id == style_layer_id && e.allocation_id() == alloc_id
                    })
                }) {
                    buffer_pool.update_feature_metadata(queue, entry, gpu_scratch);
                }
                buffer_pool.bump_layer_paint_revision(coords, &style_layer.id);
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

fn evaluate_circle_metadata_into(
    out: &mut Vec<FillShaderFeatureMetadata>,
    circle_paint: &CirclePaint,
    all_props: &[FeatureProperties],
    hits: &[u32],
    verts_per_circle: usize,
    tile_zoom: f64,
    (seq, style_layer_id, cache): (u64, crate::style::StyleLayerId, &mut EvalCache),
) {
    out.clear();
    let needed = hits.len() * verts_per_circle * 2;
    if out.capacity() < needed {
        out.reserve(needed - out.len());
    }

    let cache_key = (seq, style_layer_id, tile_zoom.to_bits());
    if cache.key != Some(cache_key) {
        cache.slots.clear();
        cache.slots.resize(all_props.len(), None);
        cache.key = Some(cache_key);
    } else if cache.slots.len() != all_props.len() {
        cache.slots.resize(all_props.len(), None);
    }

    let radius_prop = circle_paint.radius();
    let radius_const = radius_prop.is_feature_constant().then(|| {
        radius_prop
            .evaluate_at_zoom(tile_zoom)
            .unwrap_or(CirclePaint::DEFAULT_RADIUS)
            .max(0.0)
    });
    let radius_ramp = ScalarRamp::compile(&radius_prop);
    let radius_zoom_const = radius_prop.is_zoom_constant();

    let color_prop = circle_paint.circle_color.as_ref();
    let color_const = match color_prop {
        None => Some([0.0, 0.0, 0.0, 1.0]),
        Some(prop) if prop.is_feature_constant() => Some(
            prop.evaluate_at_zoom(tile_zoom)
                .map_or([0.0, 0.0, 0.0, 1.0], |c| c.to_array()),
        ),
        Some(_) => None,
    };
    let color_ramp = color_prop.and_then(ColorRamp::compile);

    let opacity_prop = circle_paint.circle_opacity.as_ref();
    let opacity_const = match opacity_prop {
        None => Some(1.0_f32),
        Some(prop) if prop.is_feature_constant() => Some(
            prop.evaluate_at_zoom(tile_zoom)
                .unwrap_or(1.0)
                .clamp(0.0, 1.0),
        ),
        Some(_) => None,
    };
    let opacity_ramp = opacity_prop.and_then(ScalarRamp::compile);

    for &feat_idx in hits {
        let idx = feat_idx as usize;
        if let Some(pair) = cache.slots.get(idx).and_then(Option::as_ref) {
            for _ in 0..verts_per_circle {
                out.extend_from_slice(pair);
            }
            continue;
        }
        let Some(props) = all_props.get(idx) else {
            continue;
        };
        let mut rgba = color_const.unwrap_or_else(|| {
            color_ramp
                .as_ref()
                .and_then(|r| r.eval(props))
                .or_else(|| {
                    color_prop
                        .and_then(|p| p.evaluate_for(props, tile_zoom))
                        .map(|c| c.to_array())
                })
                .unwrap_or([0.0, 0.0, 0.0, 1.0])
        });
        let opacity = opacity_const.unwrap_or_else(|| {
            opacity_ramp
                .as_ref()
                .and_then(|r| r.eval(props))
                .or_else(|| opacity_prop.and_then(|p| p.evaluate_for(props, tile_zoom)))
                .unwrap_or(1.0)
                .clamp(0.0, 1.0)
        });
        rgba[3] *= opacity;

        let (radius, radius_next) = if let Some(r) = radius_const {
            (r, r)
        } else {
            let r0 = radius_ramp
                .as_ref()
                .and_then(|r| r.eval(props))
                .or_else(|| radius_prop.evaluate_for(props, tile_zoom))
                .unwrap_or(CirclePaint::DEFAULT_RADIUS)
                .max(0.0);
            let r1 = if radius_zoom_const {
                r0
            } else {
                radius_prop
                    .evaluate_for(props, tile_zoom + 1.0)
                    .unwrap_or(CirclePaint::DEFAULT_RADIUS)
                    .max(0.0)
            };
            (r0, r1)
        };

        let pair = [
            FillShaderFeatureMetadata::from_rgba(rgba),
            pack_dynamic_circle_radii(radius, radius_next),
        ];
        if let Some(slot) = cache.slots.get_mut(idx) {
            *slot = Some(pair);
        }
        for _ in 0..verts_per_circle {
            out.extend_from_slice(&pair);
        }
    }
}

fn fallback_dynamic_circle_from_vertices(
    out: &mut Vec<FillShaderFeatureMetadata>,
    feature_indices: &[u32],
    active_colors: &[[f32; 4]],
    fallback_color: [f32; 4],
    vertices: &[crate::render::ShaderVertex],
) {
    out.clear();
    let needed = vertices.len() * 2;
    if out.capacity() < needed {
        out.reserve(needed - out.len());
    }
    let mut vert_cursor = 0_usize;
    for (feat_idx, &count) in feature_indices.iter().enumerate() {
        let color = active_colors
            .get(feat_idx)
            .copied()
            .unwrap_or(fallback_color);
        let fill_meta = FillShaderFeatureMetadata::from_rgba(color);
        for _ in 0..count {
            let Some(v) = vertices.get(vert_cursor) else {
                break;
            };
            vert_cursor += 1;
            out.extend_from_slice(&[
                fill_meta,
                pack_dynamic_circle_radii(v.normal[0], v.edge_distance),
            ]);
        }
    }
    while vert_cursor < vertices.len() {
        let v = &vertices[vert_cursor];
        vert_cursor += 1;
        out.extend_from_slice(&[
            FillShaderFeatureMetadata::from_rgba(fallback_color),
            pack_dynamic_circle_radii(v.normal[0], v.edge_distance),
        ]);
    }
}
