//! Single-bucket GPU geometry + layer/feature metadata upload helper.

use std::sync::Arc;

use super::{
    metadata::{
        feature_metadata_into, metadata_for_layer, pattern_runs, sort_runs,
        uses_layer_metadata_uniform_color,
    },
    paint, DynPoints, VectorPaintFrame,
};
use crate::{
    render::shaders::{FillShaderFeatureMetadata, Vec4f32},
    style::{layer::StyleLayer, Style},
    vector::{AvailableVectorLayerBucket, VectorBufferPool},
};

/// Caches one `Arc<StyleLayer>` per style layer for the active `style.revision()` so resident
/// [`IndexEntry`](crate::vector::IndexEntry) records share definitions instead of deep-cloning
/// per tile.
#[derive(Default)]
pub(super) struct SharedStyleLayers {
    revision: Option<u64>,
    layers: Vec<Arc<StyleLayer>>,
}

impl SharedStyleLayers {
    pub(super) fn sync(&mut self, style: &Style) -> &[Arc<StyleLayer>] {
        let rev = style.revision();
        if self.revision != Some(rev) || self.layers.len() != style.layers.len() {
            self.revision = Some(rev);
            self.layers.clear();
            self.layers
                .extend(style.layers.iter().map(|layer| Arc::new(layer.clone())));
        }
        &self.layers
    }
}

pub(super) enum UploadOutcome {
    Uploaded,
    PoolSaturated,
    Failed,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn upload_bucket(
    buffer_pool: &mut VectorBufferPool,
    queue: &crate::render::upload_queue::UploadQueue,
    style_layer: &Arc<StyleLayer>,
    bucket: &AvailableVectorLayerBucket,
    fallback_vertices: Option<&[crate::render::ShaderVertex]>,
    (paint_entries, overscaled_zoom, is_dynamic_circle): (
        Option<&[crate::vector::feature_paint::BucketFeatureEntry]>,
        u8,
        bool,
    ),
    (spatial, paint): (
        &[crate::vector::structures::SpatialBuffer],
        VectorPaintFrame,
    ),
    (protected, feature_metadata, dyn_points): (
        &rustc_hash::FxHashSet<crate::coords::WorldTileCoords>,
        &mut Vec<FillShaderFeatureMetadata>,
        &mut DynPoints,
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

    let fallback_color = color.unwrap_or([0.0, 0.0, 0.0, 1.0]);
    let uniform = (!is_dynamic_circle)
        .then(|| paint::uniform_color(style_layer, f64::from(zoom)))
        .flatten();
    let evaluated_colors;
    let active_colors: &[[f32; 4]] = match (uniform, paint_entries) {
        (None, Some(entries)) => {
            evaluated_colors = crate::vector::feature_paint::evaluate_bucket_colors(
                style_layer,
                entries,
                f64::from(zoom),
            );
            &evaluated_colors
        }
        _ => feature_colors,
    };

    #[cfg(feature = "geojson")]
    let dynamic_seq = if is_dynamic_circle {
        crate::geojson::update::dynamic_points::populate_dynamic_circle_bucket(
            Some(dyn_points),
            style_layer,
            bucket,
            fallback_vertices,
            active_colors,
            fallback_color,
            overscaled_zoom,
            feature_metadata,
        )
    } else {
        None
    };
    #[cfg(not(feature = "geojson"))]
    let _ = (dyn_points, overscaled_zoom);

    if !is_dynamic_circle && uniform.is_none() {
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
        .and_then(crate::vector::pattern::per_feature_pattern)
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
                Arc::clone(style_layer),
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
                Arc::clone(style_layer),
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
            Arc::clone(style_layer),
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
    buffer_pool.set_layer_evaluated_zoom(coords, &style_layer.id, zoom);
    let opaque_fill = style_layer.type_ == "fill"
        && !matches!(
            style_layer.paint.as_ref(),
            Some(crate::style::layer::LayerPaint::Fill(f)) if f.fill_pattern.is_some()
        )
        && if let Some(color) = uniform {
            color[3] >= 1.0
        } else {
            !feature_metadata.is_empty() && feature_metadata.iter().all(|fm| fm.color[3] == 255)
        };
    buffer_pool.set_layer_opaque_color(coords, &style_layer.id, opaque_fill);
    #[cfg(feature = "geojson")]
    if is_dynamic_circle {
        crate::geojson::update::dynamic_points::record_uploaded_dynamic_circle(
            Some(dyn_points),
            buffer_pool,
            coords,
            style_layer,
            dynamic_seq,
        );
    }
    UploadOutcome::Uploaded
}
