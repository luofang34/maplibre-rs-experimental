//! Single-bucket GPU geometry + layer/feature metadata upload helper.

use std::sync::Arc;

use super::{
    metadata::{feature_metadata_into, metadata_for_layer, pattern_runs, sort_runs},
    DynPoints, VectorPaintFrame,
};
use crate::{
    coords::WorldTileCoords,
    render::{
        shaders::{FillShaderFeatureMetadata, ShaderLayerMetadata, Vec4f32},
        upload_queue::UploadQueue,
        ShaderVertex,
    },
    style::{layer::StyleLayer, Style},
    vector::{
        feature_paint::BucketFeatureEntry,
        structures::SpatialBuffer,
        uniform_paint::{uniform_color, uses_layer_metadata_uniform_color},
        AllocationError, AvailableVectorLayerBucket, LayerGeometryUpload, UniformLayerUpload,
        VectorBufferPool,
    },
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
    Uploaded(Option<((u16, u64), ShaderLayerMetadata)>),
    PoolSaturated,
    Failed,
}

pub(super) struct BucketUploadInput<'a> {
    pub style_layer: &'a Arc<StyleLayer>,
    pub bucket: &'a AvailableVectorLayerBucket,
    pub fallback_vertices: Option<&'a [ShaderVertex]>,
    pub paint_entries: Option<&'a [BucketFeatureEntry]>,
    pub overscaled_zoom: u8,
    pub is_dynamic_circle: bool,
    pub spatial: &'a [SpatialBuffer],
    pub paint: VectorPaintFrame,
    pub protected: &'a rustc_hash::FxHashSet<WorldTileCoords>,
}

pub(super) struct BucketUploadScratch<'a> {
    pub feature_metadata: &'a mut Vec<FillShaderFeatureMetadata>,
    pub dyn_points: &'a mut DynPoints,
}

pub(super) fn upload_bucket(
    buffer_pool: &mut VectorBufferPool,
    queue: &UploadQueue,
    input: BucketUploadInput<'_>,
    scratch: BucketUploadScratch<'_>,
) -> UploadOutcome {
    let zoom = input.paint.zoom;
    let uniform = (!input.is_dynamic_circle)
        .then(|| uniform_color(input.style_layer, f64::from(zoom)))
        .flatten();
    let (evaluated_colors, dynamic_seq) = prepare_feature_metadata(
        &input,
        uniform,
        scratch.feature_metadata,
        scratch.dyn_points,
    );
    let active_colors: &[[f32; 4]] = evaluated_colors
        .as_deref()
        .unwrap_or(&input.bucket.feature_colors);
    let buffer = input
        .spatial
        .iter()
        .find(|(tile, id, _)| *tile == input.bucket.coords && *id == input.style_layer.id)
        .map_or(&input.bucket.buffer, |(_, _, spatial)| spatial);
    let layer_metadata = metadata_for_layer(input.style_layer, input.bucket.coords, input.paint);
    let runs = input
        .style_layer
        .paint
        .as_ref()
        .and_then(crate::vector::pattern::per_feature_pattern)
        .map(|_| {
            pattern_runs(
                &input.bucket.feature_indices,
                active_colors,
                &buffer.indices,
            )
        });
    let sort_runs = (input.style_layer.type_ == "circle"
        && !input.bucket.feature_sort_keys.is_empty())
    .then(|| {
        sort_runs(
            &input.bucket.feature_indices,
            &input.bucket.feature_sort_keys,
            &buffer.indices,
        )
    });

    tracing::debug!(coords = %input.bucket.coords, "allocating vector geometry");
    if let Err(error) = replace_bucket_in_pool(
        buffer_pool,
        queue,
        &input,
        buffer,
        layer_metadata,
        uniform,
        scratch.feature_metadata,
    ) {
        return handle_upload_error(input.bucket.coords, error);
    }

    finalize_uploaded_bucket(
        buffer_pool,
        &input,
        BucketFinalizeMeta {
            uniform,
            feature_metadata: scratch.feature_metadata,
            runs,
            sort_runs,
            layer_metadata,
            dynamic_seq,
        },
        scratch.dyn_points,
    )
}

fn prepare_feature_metadata(
    input: &BucketUploadInput<'_>,
    uniform: Option<Vec4f32>,
    feature_metadata: &mut Vec<FillShaderFeatureMetadata>,
    dyn_points: &mut DynPoints,
) -> (Option<Vec<[f32; 4]>>, Option<u64>) {
    let zoom = input.paint.zoom;
    let color: Option<Vec4f32> = input
        .style_layer
        .paint
        .as_ref()
        .and_then(|paint| paint.color_at(f64::from(zoom)))
        .map(Into::into);
    let fallback_color = color.unwrap_or([0.0, 0.0, 0.0, 1.0]);
    let evaluated_colors = match (uniform, input.paint_entries) {
        (None, Some(entries)) => Some(crate::vector::feature_paint::evaluate_bucket_colors(
            input.style_layer,
            entries,
            f64::from(zoom),
        )),
        _ => None,
    };
    let active_colors: &[[f32; 4]] = evaluated_colors
        .as_deref()
        .unwrap_or(&input.bucket.feature_colors);

    #[cfg(feature = "geojson")]
    let dynamic_seq = if input.is_dynamic_circle {
        super::dynamic_points::populate_dynamic_circle_bucket(
            super::dynamic_points::DynamicCircleBucketInput {
                dyn_points: Some(dyn_points),
                style_layer: input.style_layer,
                bucket: input.bucket,
                fallback_vertices: input.fallback_vertices,
                active_colors,
                fallback_color,
                overscaled_zoom: input.overscaled_zoom,
            },
            feature_metadata,
        )
    } else {
        None
    };
    #[cfg(not(feature = "geojson"))]
    let dynamic_seq: Option<u64> = {
        let _ = dyn_points;
        None
    };

    if !input.is_dynamic_circle && uniform.is_none() {
        feature_metadata_into(
            feature_metadata,
            input.style_layer,
            &input.bucket.feature_indices,
            active_colors,
            fallback_color,
        );
    }
    (evaluated_colors, dynamic_seq)
}

fn replace_bucket_in_pool(
    buffer_pool: &mut VectorBufferPool,
    queue: &UploadQueue,
    input: &BucketUploadInput<'_>,
    geometry: &crate::vector::tessellation::OverAlignedVertexBuffer<
        ShaderVertex,
        crate::vector::tessellation::IndexDataType,
    >,
    layer_metadata: ShaderLayerMetadata,
    uniform: Option<Vec4f32>,
    feature_metadata: &[FillShaderFeatureMetadata],
) -> Result<(), AllocationError> {
    let coords = input.bucket.coords;
    if let Some(color) = uniform {
        let records = input
            .bucket
            .feature_indices
            .iter()
            .map(|&c| c as usize)
            .sum();
        let upload = UniformLayerUpload {
            style_layer: Arc::clone(input.style_layer),
            geometry,
            layer_metadata,
            uniform_feature: FillShaderFeatureMetadata::from_rgba(color),
            records,
        };
        if uses_layer_metadata_uniform_color(input.style_layer) {
            buffer_pool.replace_layer_geometry_layer_uniform_shared_unless(
                queue,
                coords,
                upload,
                input.fallback_vertices,
                |candidate| input.protected.contains(&candidate),
            )
        } else {
            buffer_pool.replace_layer_geometry_uniform_shared_unless(
                queue,
                coords,
                upload,
                input.fallback_vertices,
                |candidate| input.protected.contains(&candidate),
            )
        }
    } else {
        buffer_pool.replace_layer_geometry_unless(
            queue,
            coords,
            LayerGeometryUpload {
                style_layer: Arc::clone(input.style_layer),
                geometry,
                layer_metadata,
                feature_metadata,
            },
            |candidate| input.protected.contains(&candidate),
        )
    }
}

fn handle_upload_error(coords: WorldTileCoords, error: AllocationError) -> UploadOutcome {
    if matches!(error, AllocationError::Capacity { .. }) {
        tracing::debug!(
            %coords,
            %error,
            "vector buffer pool saturated by higher-priority tiles; skipping peripheral upload"
        );
        return UploadOutcome::PoolSaturated;
    }
    tracing::error!(%coords, %error, "tile geometry upload failed");
    UploadOutcome::Failed
}

struct BucketFinalizeMeta<'a> {
    uniform: Option<Vec4f32>,
    feature_metadata: &'a [FillShaderFeatureMetadata],
    runs: Option<crate::vector::resource::PatternRuns>,
    sort_runs: Option<crate::vector::resource::SortRuns>,
    layer_metadata: ShaderLayerMetadata,
    dynamic_seq: Option<u64>,
}

fn finalize_uploaded_bucket(
    buffer_pool: &mut VectorBufferPool,
    input: &BucketUploadInput<'_>,
    meta: BucketFinalizeMeta<'_>,
    dyn_points: &mut DynPoints,
) -> UploadOutcome {
    let BucketFinalizeMeta {
        uniform,
        feature_metadata,
        runs,
        sort_runs,
        layer_metadata,
        dynamic_seq,
    } = meta;
    let coords = input.bucket.coords;
    buffer_pool.set_pattern_runs(coords, &input.style_layer.id, runs);
    buffer_pool.set_sort_runs(coords, &input.style_layer.id, sort_runs);
    buffer_pool.set_layer_evaluated_zoom(coords, &input.style_layer.id, input.paint.zoom);
    let opaque_fill = input.style_layer.type_ == "fill"
        && !matches!(
            input.style_layer.paint.as_ref(),
            Some(crate::style::layer::LayerPaint::Fill(f)) if f.fill_pattern.is_some()
        )
        && if let Some(color) = uniform {
            color[3] >= 1.0
        } else {
            !feature_metadata.is_empty() && feature_metadata.iter().all(|fm| fm.color[3] == 255)
        };
    buffer_pool.set_layer_opaque_color(coords, &input.style_layer.id, opaque_fill);
    let slot = buffer_pool
        .as_ref()
        .get_layers(coords)
        .and_then(|layers| {
            layers
                .iter()
                .rev()
                .find(|entry| entry.style_layer_id == input.style_layer.layer_id())
        })
        .and_then(|entry| {
            let range = entry.layer_metadata_buffer_range();
            (range.end > range.start).then_some((entry.page(), range.start))
        });
    #[cfg(feature = "geojson")]
    if input.is_dynamic_circle {
        super::dynamic_points::record_uploaded_dynamic_circle(
            Some(dyn_points),
            buffer_pool,
            coords,
            input.style_layer,
            dynamic_seq,
        );
    }
    #[cfg(not(feature = "geojson"))]
    let _ = (dyn_points, dynamic_seq);
    UploadOutcome::Uploaded(slot.map(|slot| (slot, layer_metadata)))
}
