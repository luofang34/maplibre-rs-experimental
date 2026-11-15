//! Uploads data to the GPU which is needed for rendering.

use crate::{
    context::MapContext,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        memory_budget::{FrameUploads, UploadKind},
        shaders::{FillShaderFeatureMetadata, Vec4f32},
        Renderer,
    },
    style::Style,
    tcs::{
        system::{SystemError, SystemResult},
        tiles::Tiles,
    },
    vector::{
        AvailableVectorLayerBucket, VectorBufferPool, VectorLayerBucket, VectorLayerBucketComponent,
    },
};

mod metadata;
pub(crate) mod paint;
mod sources;
pub(crate) mod style_refresh;

use metadata::{
    feature_metadata_into, metadata_for_layer, sort_runs, upload_size,
    uses_layer_metadata_uniform_color,
};

pub(crate) fn drape_paint_zoom(zoom: f64) -> f64 {
    (zoom * 8.0).round() / 8.0
}

#[derive(Clone, Copy, PartialEq)]
pub(super) struct VectorPaintFrame {
    zoom: f32,
    bearing: f32,
    /// The root light as extruded polygons are shaded with it.
    light: crate::style::light::ExtrusionLight,
}

pub fn upload_system(
    MapContext {
        world,
        style,
        view_state,
        renderer: Renderer {
            queue, settings, ..
        },
        ..
    }: &mut MapContext,
) -> SystemResult {
    let Some(pool_state) = world.resources.get::<Eventually<VectorBufferPool>>() else {
        return Ok(());
    };
    if !matches!(pool_state, Initialized(_)) {
        return Err(SystemError::Dependencies);
    }
    if settings.buffer_pools.is_empty()
        || crate::render::eye_covering::EyeInFrame::reuses_content(world)
    {
        return Ok(());
    }
    let (paint_frame, previous_paint) = frame_paint(world, style, view_state);
    let refresh_paint = previous_paint != Some(paint_frame);
    let (source_tiles, painted_tiles) =
        sources::frame_sources_and_painted(world, style, view_state)?;
    let (spatial, changed) = super::structures::for_frame(world, style, &painted_tiles);
    world.resources.get_or_init_mut::<FrameUploads>();
    style_refresh::apply_dirty_style(world, style, queue, paint_frame);
    let Some((Initialized(buffer_pool), budget)) = world
        .resources
        .query_mut::<(&mut Eventually<VectorBufferPool>, &mut FrameUploads)>()
    else {
        return Err(SystemError::Dependencies);
    };
    if refresh_paint {
        paint::refresh_layer_paint(
            buffer_pool,
            queue,
            &painted_tiles,
            paint_frame,
            previous_paint,
        );
    }
    if changed {
        super::structures::refresh_gpu(buffer_pool, queue, &spatial, &world.tiles);
    }
    let allow_cpu_release =
        cfg!(target_arch = "wasm32") && settings.buffer_pools.vertices >= 1_000_000;
    let (deferred, needs_refetch) = upload_tessellated_layer(
        buffer_pool,
        queue,
        &mut world.tiles,
        style,
        source_tiles,
        (&spatial, paint_frame),
        (budget, allow_cpu_release),
    );
    if deferred {
        crate::render::frame_signals::keep_animating(world);
    }
    if needs_refetch {
        crate::render::frame_signals::mark_dirty(world);
    }
    Ok(())
}

fn frame_paint(
    world: &mut crate::tcs::world::World,
    style: &Style,
    view_state: &crate::render::view_state::ViewState,
) -> (VectorPaintFrame, Option<VectorPaintFrame>) {
    let zoom = if style.terrain.is_some() {
        paint::stabilize_zoom(world, view_state.style_zoom().value()) as f32
    } else {
        view_state.style_zoom().level()
    };
    let bearing = view_state.camera().get_bearing().0 as f32;
    let light = style
        .light
        .clone()
        .unwrap_or_default()
        .extrusion_light(view_state.style_zoom().value(), f64::from(bearing))
        .unwrap_or_default();
    let paint_frame = VectorPaintFrame {
        zoom,
        bearing,
        light,
    };
    let previous_paint = world.resources.get::<VectorPaintFrame>().copied();
    world.resources.insert(paint_frame);
    (paint_frame, previous_paint)
}

#[cfg_attr(not(target_arch = "wasm32"), allow(unused_variables))]
fn upload_tessellated_layer(
    buffer_pool: &mut VectorBufferPool,
    queue: &crate::render::upload_queue::UploadQueue,
    tiles: &mut Tiles,
    style: &Style,
    source_tiles: Vec<crate::coords::WorldTileCoords>,
    (spatial, paint): (&[super::structures::SpatialBuffer], VectorPaintFrame),
    (budget, allow_cpu_release): (&mut FrameUploads, bool),
) -> (bool, bool) {
    #[cfg(target_arch = "wasm32")]
    let has_dem_source = crate::terrain::source::dem_source(style).is_some();
    let mut feature_meta_scratch = Vec::new();
    let mut needs_refetch = false;
    let protected: std::collections::HashSet<_> = source_tiles.iter().copied().collect();
    for coords in source_tiles {
        super::content::ensure(tiles, coords);
        if tiles
            .query::<(
                &VectorLayerBucketComponent,
                &super::content::LayerReplacements,
            )>(coords)
            .is_none()
        {
            continue;
        }
        let mut uploaded = false;
        #[cfg(target_arch = "wasm32")]
        let mut pending_cpu_releases: Vec<(String, Option<u64>, bool)> = Vec::new();
        for style_layer in &style.layers {
            enum Step {
                Skip,
                Exhausted,
                Saturated,
                Commit {
                    replacement: Option<usize>,
                    did_upload: bool,
                    #[cfg(target_arch = "wasm32")]
                    release_cpu: Option<Option<u64>>,
                },
            }
            let step = {
                let Some((layers, pending)) = tiles.query::<(
                    &VectorLayerBucketComponent,
                    &super::content::LayerReplacements,
                )>(coords) else {
                    break;
                };
                let replacement = pending
                    .vector
                    .iter()
                    .position(|layer| layer.style_layer_id == style_layer.id);
                let bucket = replacement.map(|index| &pending.vector[index]).or_else(|| {
                    layers.layers.iter().find_map(|layer| match layer {
                        VectorLayerBucket::AvailableLayer(layer)
                            if layer.style_layer_id == style_layer.id =>
                        {
                            Some(layer)
                        }
                        _ => None,
                    })
                });
                let Some(bucket) = bucket else {
                    continue;
                };
                if replacement.is_none() && buffer_pool.has_layer(coords, &style_layer.id) {
                    continue;
                }
                let is_uniform = paint::uniform_color(style_layer, f64::from(paint.zoom)).is_some();
                if bucket.buffer.usable_indices == 0 && bucket.buffer.buffer.indices.is_empty() {
                    buffer_pool.remove_layer(coords, &style_layer.id);
                    Step::Commit {
                        replacement,
                        did_upload: false,
                        #[cfg(target_arch = "wasm32")]
                        release_cpu: None,
                    }
                } else if bucket.buffer.buffer.indices.is_empty() {
                    if !style_layer.is_hidden() {
                        needs_refetch = true;
                    }
                    continue;
                } else {
                    let bytes = upload_size(style_layer, bucket, is_uniform);
                    if budget.geometry_tiles_exhausted()
                        || !budget.can_take(UploadKind::Geometry, bytes)
                    {
                        Step::Exhausted
                    } else {
                        #[cfg(target_arch = "wasm32")]
                        let release_cpu = (allow_cpu_release
                            && metadata::can_release_cpu_geometry(
                                style_layer,
                                bucket,
                                has_dem_source,
                            ))
                        .then(|| bucket.buffer.shared_geometry_key());
                        let fallback_vertices =
                            metadata::find_shared_fallback_vertices(bucket, layers, pending);
                        let paint_store =
                            tiles.query::<&super::feature_paint::TileFeaturePaintComponent>(coords);
                        let paint_entries = if replacement.is_some() {
                            paint_store
                                .and_then(|s| s.pending.get(&style_layer.id))
                                .map(Vec::as_slice)
                        } else {
                            paint_store
                                .and_then(|s| s.committed.get(&style_layer.id))
                                .map(Vec::as_slice)
                        };
                        match upload_bucket(
                            buffer_pool,
                            queue,
                            style_layer,
                            bucket,
                            fallback_vertices,
                            (paint_entries, layers.overscaled_zoom),
                            (spatial, paint),
                            (&protected, &mut feature_meta_scratch),
                        ) {
                            UploadOutcome::Uploaded => {
                                budget.take(UploadKind::Geometry, bytes);
                                Step::Commit {
                                    replacement,
                                    did_upload: true,
                                    #[cfg(target_arch = "wasm32")]
                                    release_cpu,
                                }
                            }
                            UploadOutcome::PoolSaturated => Step::Saturated,
                            UploadOutcome::Failed => Step::Skip,
                        }
                    }
                }
            };
            match step {
                Step::Skip | Step::Saturated => continue,
                Step::Exhausted => {
                    #[cfg(target_arch = "wasm32")]
                    metadata::flush_safe_cpu_releases(
                        tiles,
                        buffer_pool,
                        coords,
                        &pending_cpu_releases,
                    );
                    return (true, needs_refetch);
                }
                Step::Commit {
                    replacement,
                    did_upload,
                    #[cfg(target_arch = "wasm32")]
                    release_cpu,
                } => {
                    uploaded |= did_upload;
                    if let Some(index) = replacement {
                        super::content::commit_pending_vector(tiles, coords, index);
                    }
                    #[cfg(target_arch = "wasm32")]
                    if let Some(shared_key) = release_cpu {
                        let release_colors =
                            !crate::style::feature_state::layer_paint_reads_feature_state(
                                style_layer,
                            );
                        pending_cpu_releases.push((
                            style_layer.id.clone(),
                            shared_key,
                            release_colors,
                        ));
                    }
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        metadata::flush_safe_cpu_releases(tiles, buffer_pool, coords, &pending_cpu_releases);
        if uploaded {
            budget.count_geometry_tile();
        }
    }
    (false, needs_refetch)
}

enum UploadOutcome {
    Uploaded,
    PoolSaturated,
    Failed,
}

#[allow(clippy::too_many_arguments)]
fn upload_bucket(
    buffer_pool: &mut VectorBufferPool,
    queue: &crate::render::upload_queue::UploadQueue,
    style_layer: &crate::style::layer::StyleLayer,
    bucket: &AvailableVectorLayerBucket,
    fallback_vertices: Option<&[crate::render::ShaderVertex]>,
    (paint_entries, overscaled_zoom): (Option<&[super::feature_paint::BucketFeatureEntry]>, u8),
    (spatial, paint): (&[super::structures::SpatialBuffer], VectorPaintFrame),
    (protected, feature_metadata): (
        &std::collections::HashSet<crate::coords::WorldTileCoords>,
        &mut Vec<FillShaderFeatureMetadata>,
    ),
) -> UploadOutcome {
    let VectorPaintFrame { zoom, .. } = paint;
    let AvailableVectorLayerBucket {
        coords,
        buffer,
        feature_indices,
        feature_colors,
        feature_sort_keys,
        ..
    } = bucket;
    let coords = *coords;
    let color: Option<Vec4f32> = style_layer
        .paint
        .as_ref()
        .and_then(|paint| paint.get_color())
        .map(|color| color.into());

    // Assign every feature in the layer the color from the style if no parsed feature_color exist.
    let fallback_color = color.unwrap_or([0.0, 0.0, 0.0, 1.0]);
    let evaluated_colors;
    let active_colors: &[[f32; 4]] = match (
        paint::uniform_color(style_layer, f64::from(zoom)),
        paint_entries,
    ) {
        (None, Some(entries)) => {
            let style_zoom = f64::from(overscaled_zoom.max(u8::from(coords.z)));
            evaluated_colors =
                super::feature_paint::evaluate_bucket_colors(style_layer, entries, style_zoom);
            &evaluated_colors
        }
        _ => feature_colors,
    };

    let uniform = paint::uniform_color(style_layer, f64::from(zoom));
    if uniform.is_none() {
        feature_metadata_into(
            feature_metadata,
            style_layer,
            feature_indices,
            active_colors,
            fallback_color,
        );
    }

    let buffer = spatial
        .iter()
        .find(|(tile, id, _)| *tile == coords && *id == style_layer.id)
        .map_or(buffer, |(_, _, spatial)| spatial);
    let layer_metadata = metadata_for_layer(style_layer, coords, paint);
    let runs = style_layer
        .paint
        .as_ref()
        .and_then(super::pattern::per_feature_pattern)
        .map(|_| pattern_runs(feature_indices, active_colors, &buffer.buffer.indices));

    let sort_runs = (style_layer.type_ == "circle" && !feature_sort_keys.is_empty())
        .then(|| sort_runs(feature_indices, feature_sort_keys, &buffer.buffer.indices));

    tracing::debug!(%coords, "allocating vector geometry");
    let upload_res = if let Some(color) = uniform {
        let encoded = FillShaderFeatureMetadata::from_rgba(color);
        let records = feature_indices.iter().map(|&c| c as usize).sum();
        if uses_layer_metadata_uniform_color(style_layer) {
            buffer_pool.replace_layer_geometry_layer_uniform_shared_unless(
                queue,
                coords,
                style_layer.clone(),
                buffer,
                fallback_vertices,
                layer_metadata,
                encoded,
                records,
                |candidate| protected.contains(&candidate),
            )
        } else {
            buffer_pool.replace_layer_geometry_uniform_shared_unless(
                queue,
                coords,
                style_layer.clone(),
                buffer,
                fallback_vertices,
                layer_metadata,
                encoded,
                records,
                |candidate| protected.contains(&candidate),
            )
        }
    } else {
        buffer_pool.replace_layer_geometry_unless(
            queue,
            coords,
            style_layer.clone(),
            buffer,
            layer_metadata,
            feature_metadata,
            |candidate| protected.contains(&candidate),
        )
    };
    if let Err(error) = upload_res {
        if matches!(
            error,
            crate::vector::resource::AllocationError::Capacity { .. }
        ) {
            tracing::debug!(
                %coords,
                %error,
                "vector buffer pool saturated by higher-priority tiles; skipping peripheral upload"
            );
            return UploadOutcome::PoolSaturated;
        }
        tracing::error!(%coords, %error, "tile geometry upload failed");
        return UploadOutcome::Failed;
    }
    buffer_pool.set_pattern_runs(coords, &style_layer.id, runs);
    buffer_pool.set_sort_runs(coords, &style_layer.id, sort_runs);
    UploadOutcome::Uploaded
}

/// The index range of each run of consecutive features that share a pattern key.
pub(super) fn pattern_runs(
    feature_indices: &[u32],
    feature_colors: &[[f32; 4]],
    indices: &[u32],
) -> Vec<(u32, std::ops::Range<u32>)> {
    let mut runs: Vec<(u32, std::ops::Range<u32>)> = Vec::new();
    let (mut vertex_end, mut position) = (0_u32, 0_u32);
    for (feature, count) in feature_indices.iter().enumerate() {
        vertex_end = vertex_end.wrapping_add(*count);
        let first = position;
        while indices
            .get(position as usize)
            .is_some_and(|index| *index < vertex_end)
        {
            position = position.wrapping_add(1);
        }
        let key = feature_colors
            .get(feature)
            .and_then(|color| crate::style::pattern_key::key_of_value(color[0]));
        let Some(key) = key.filter(|_| position > first) else {
            continue;
        };
        match runs.last_mut() {
            Some((last, range)) if *last == key && range.end == first => range.end = position,
            _ => runs.push((key, first..position)),
        }
    }
    runs
}

#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "headless", not(target_arch = "wasm32")))]
#[path = "upload_system/retry/tests.rs"]
mod retry_tests;
