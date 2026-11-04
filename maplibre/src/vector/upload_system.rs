//! Uploads data to the GPU which is needed for rendering.

use crate::{
    context::MapContext,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        memory_budget::{FrameUploads, UploadKind},
        shaders::{FillShaderFeatureMetadata, Vec4f32},
        Renderer,
    },
    style::{layer::LayerPaint, Style},
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

use metadata::{feature_metadata, metadata_for_layer};

pub(crate) fn drape_paint_zoom(zoom: f64) -> f64 {
    (zoom * 8.0).round() / 8.0
}

#[derive(Clone, Copy, PartialEq)]
struct VectorPaintFrame {
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
    let Some((Initialized(buffer_pool), budget)) = world
        .resources
        .query_mut::<(&mut Eventually<VectorBufferPool>, &mut FrameUploads)>()
    else {
        return Err(SystemError::Dependencies);
    };
    if refresh_paint {
        refresh_layer_paint(
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
    let deferred = upload_tessellated_layer(
        buffer_pool,
        queue,
        &mut world.tiles,
        style,
        source_tiles,
        (&spatial, paint_frame),
        budget,
    );
    if deferred {
        crate::render::frame_signals::keep_animating(world);
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

fn refresh_layer_paint(
    buffer_pool: &VectorBufferPool,
    queue: &crate::render::upload_queue::UploadQueue,
    source_tiles: &[crate::coords::WorldTileCoords],
    frame: VectorPaintFrame,
    previous: Option<VectorPaintFrame>,
) {
    let VectorPaintFrame { zoom, .. } = frame;
    for coords in source_tiles {
        for entry in buffer_pool
            .index()
            .get_layers(*coords)
            .into_iter()
            .flatten()
        {
            let metadata = metadata_for_layer(&entry.style_layer, *coords, frame);
            let unchanged = previous.is_some_and(|previous| {
                bytemuck::bytes_of(&metadata_for_layer(&entry.style_layer, *coords, previous))
                    == bytemuck::bytes_of(&metadata)
            });
            if !unchanged {
                buffer_pool.update_layer_metadata(queue, entry, metadata);
            }
            if let Some(color) = paint::uniform_color(&entry.style_layer, f64::from(zoom)) {
                if previous
                    .map(|frame| frame.zoom)
                    .and_then(|zoom| paint::uniform_color(&entry.style_layer, f64::from(zoom)))
                    == Some(color)
                {
                    continue;
                }
                let count = (entry.feature_metadata_buffer_range().end
                    - entry.feature_metadata_buffer_range().start)
                    as usize
                    / std::mem::size_of::<FillShaderFeatureMetadata>();
                let colors = vec![FillShaderFeatureMetadata { color }; count];
                buffer_pool.update_feature_metadata(queue, entry, &colors);
            }
        }
    }
}

fn upload_tessellated_layer(
    buffer_pool: &mut VectorBufferPool,
    queue: &crate::render::upload_queue::UploadQueue,
    tiles: &mut Tiles,
    style: &Style,
    source_tiles: Vec<crate::coords::WorldTileCoords>,
    (spatial, paint): (&[super::structures::SpatialBuffer], VectorPaintFrame),
    budget: &mut FrameUploads,
) -> bool {
    let mut protected = std::collections::HashSet::with_capacity(source_tiles.len());
    for coords in source_tiles {
        protected.insert(coords);
        super::content::ensure(tiles, coords);
        let Some((layers, pending)) = tiles.query_mut::<(
            &mut VectorLayerBucketComponent,
            &mut super::content::LayerReplacements,
        )>(coords) else {
            continue;
        };
        let mut uploaded = false;
        let mut saturated = false;
        for style_layer in &style.layers {
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
            let loaded = buffer_pool
                .get_loaded_style_layers_at(coords)
                .is_some_and(|layers| layers.contains(style_layer.id.as_str()));
            if replacement.is_none() && loaded {
                continue;
            }
            if bucket.buffer.buffer.indices.is_empty() {
                buffer_pool.remove_layer(coords, &style_layer.id);
            } else {
                if budget.geometry_tiles_exhausted()
                    || !budget.take(UploadKind::Geometry, upload_size(bucket))
                {
                    return true;
                }
                match upload_bucket(
                    buffer_pool,
                    queue,
                    style_layer,
                    bucket,
                    (spatial, paint),
                    &protected,
                ) {
                    UploadOutcome::Uploaded => uploaded = true,
                    UploadOutcome::PoolSaturated => {
                        saturated = true;
                        break;
                    }
                    UploadOutcome::Failed => continue,
                }
            }
            if let Some(index) = replacement {
                super::content::commit_vector(layers, pending.vector.remove(index));
            }
        }
        if uploaded {
            budget.count_geometry_tile();
        }
        if saturated {
            return false;
        }
    }
    false
}

enum UploadOutcome {
    Uploaded,
    PoolSaturated,
    Failed,
}

fn upload_size(bucket: &AvailableVectorLayerBucket) -> usize {
    bucket.buffer.buffer.vertices.len() * size_of::<crate::render::ShaderVertex>()
        + bucket.buffer.buffer.indices.len() * size_of::<u32>()
        + bucket
            .feature_indices
            .iter()
            .map(|count| *count as usize)
            .sum::<usize>()
            * size_of::<FillShaderFeatureMetadata>()
}

fn upload_bucket(
    buffer_pool: &mut VectorBufferPool,
    queue: &crate::render::upload_queue::UploadQueue,
    style_layer: &crate::style::layer::StyleLayer,
    bucket: &AvailableVectorLayerBucket,
    (spatial, paint): (&[super::structures::SpatialBuffer], VectorPaintFrame),
    protected: &std::collections::HashSet<crate::coords::WorldTileCoords>,
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

    let feature_metadata = if let Some(color) = paint::uniform_color(style_layer, f64::from(zoom)) {
        feature_metadata(feature_indices, &[], color)
    } else {
        feature_metadata(feature_indices, feature_colors, fallback_color)
    };

    let buffer = spatial
        .iter()
        .find(|(tile, id, _)| *tile == coords && *id == style_layer.id)
        .map_or(buffer, |(_, _, spatial)| spatial);
    let layer_metadata = metadata_for_layer(style_layer, coords, paint);
    let runs = style_layer
        .paint
        .as_ref()
        .and_then(super::pattern::per_feature_pattern)
        .map(|_| pattern_runs(feature_indices, feature_colors, &buffer.buffer.indices));

    let sort_runs = (style_layer.type_ == "circle" && !feature_sort_keys.is_empty())
        .then(|| sort_runs(feature_indices, feature_sort_keys, &buffer.buffer.indices));

    tracing::debug!(%coords, "allocating vector geometry");
    if let Err(error) = buffer_pool.replace_layer_geometry_unless(
        queue,
        coords,
        style_layer.clone(),
        buffer,
        layer_metadata,
        &feature_metadata,
        |candidate| protected.contains(&candidate),
    ) {
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

/// The index range of each run of consecutive features that share a sort key.
fn sort_runs(
    feature_indices: &[u32],
    feature_sort_keys: &[f32],
    indices: &[u32],
) -> Vec<(f32, std::ops::Range<u32>)> {
    let mut runs: Vec<(f32, std::ops::Range<u32>)> = Vec::new();
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
        if position == first {
            continue;
        }
        let key = feature_sort_keys.get(feature).copied().unwrap_or(0.0);
        match runs.last_mut() {
            Some((last, range)) if *last == key && range.end == first => range.end = position,
            _ => runs.push((key, first..position)),
        }
    }
    runs
}

/// The index range of each run of consecutive features that share a pattern key.
fn pattern_runs(
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

fn layer_translate_tile_units(
    paint: Option<&LayerPaint>,
    tile_zoom: crate::coords::ZoomLevel,
    view_zoom: f32,
    bearing: f32,
) -> [f32; 2] {
    let translated = crate::style::translation::layer_translate_pixels(
        paint,
        f64::from(view_zoom),
        f64::from(bearing),
    )
    .map(|pixels| pixels as f32);
    let pixels_to_tile_units = 8.0 * 2.0_f32.powf(f32::from(u8::from(tile_zoom)) - view_zoom);
    [
        translated[0] * pixels_to_tile_units,
        translated[1] * pixels_to_tile_units,
    ]
}

#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "headless", not(target_arch = "wasm32")))]
#[path = "upload_system/retry/tests.rs"]
mod retry_tests;
