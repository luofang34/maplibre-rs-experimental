use std::sync::Arc;

use super::{
    bucket::{upload_bucket, BucketUploadInput, BucketUploadScratch, UploadOutcome},
    layer_is_dynamic_circle,
    metadata::upload_size,
    paint, shared_fallback, DynPoints, VectorPaintFrame,
};
use crate::{
    coords::WorldTileCoords,
    render::{
        memory_budget::FrameUploads, shaders::FillShaderFeatureMetadata, upload_queue::UploadQueue,
    },
    style::{layer::StyleLayer, Style},
    tcs::tiles::Tiles,
    vector::{
        content::{commit_pending_vector, ensure as ensure_content, LayerReplacements},
        feature_paint::TileFeaturePaintComponent,
        structures::SpatialBuffer,
        VectorBufferPool, VectorLayerBucket, VectorLayerBucketComponent,
    },
};

pub(super) struct TessellatedUploadPlan<'a> {
    pub style: &'a Style,
    pub shared_style_layers: &'a [Arc<StyleLayer>],
    pub source_tiles: Vec<WorldTileCoords>,
    pub uncovered_sources: &'a rustc_hash::FxHashSet<WorldTileCoords>,
    pub spatial: &'a [SpatialBuffer],
    pub paint: VectorPaintFrame,
    pub allow_cpu_release: bool,
}

pub(super) struct TessellatedUploadState<'a> {
    pub budget: &'a mut FrameUploads,
    pub dyn_points: &'a mut DynPoints,
    pub paint_slots: Option<&'a mut paint::SlotMetaCache>,
}

struct TilePassScratch<'a, 'b> {
    feature_meta: Vec<FillShaderFeatureMetadata>,
    protected: rustc_hash::FxHashSet<WorldTileCoords>,
    has_dem_source: bool,
    upload_started: instant::Instant,
    time_budget: Option<std::time::Duration>,
    uploaded_any_bucket: bool,
    any_deferred: bool,
    needs_refetch: bool,
    stamp: (u64, u64),
    state: &'a mut TessellatedUploadState<'b>,
}

enum LayerStep {
    Continue,
    Incomplete,
    Exhausted,
    Commit {
        replacement: Option<usize>,
        did_upload: bool,
        release_cpu: Option<Option<u64>>,
    },
}

#[cfg(all(test, feature = "headless"))]
pub(super) fn upload_tessellated_layer(
    buffer_pool: &mut VectorBufferPool,
    queue: &UploadQueue,
    tiles: &mut Tiles,
    style: &Style,
    source_tiles: Vec<WorldTileCoords>,
    (spatial, paint): (&[SpatialBuffer], VectorPaintFrame),
    (budget, allow_cpu_release, dyn_points): (&mut FrameUploads, bool, &mut DynPoints),
) -> (bool, bool) {
    let empty = rustc_hash::FxHashSet::default();
    let shared: Vec<_> = style.layers.iter().cloned().map(Arc::new).collect();
    upload_tessellated_layer_with_uncovered(
        buffer_pool,
        queue,
        tiles,
        TessellatedUploadPlan {
            style,
            shared_style_layers: &shared,
            source_tiles,
            uncovered_sources: &empty,
            spatial,
            paint,
            allow_cpu_release,
        },
        TessellatedUploadState {
            budget,
            dyn_points,
            paint_slots: None,
        },
    )
}

pub(super) fn upload_tessellated_layer_with_uncovered(
    buffer_pool: &mut VectorBufferPool,
    queue: &UploadQueue,
    tiles: &mut Tiles,
    plan: TessellatedUploadPlan<'_>,
    mut state: TessellatedUploadState<'_>,
) -> (bool, bool) {
    let stamp = (plan.style.revision(), buffer_pool.eviction_generation());
    if all_tiles_uploaded(tiles, &plan.source_tiles, stamp) {
        return (false, false);
    }
    let protected: rustc_hash::FxHashSet<_> = plan.source_tiles.iter().copied().collect();
    let time_budget = state.budget.vector_upload_time_budget();
    let mut pass = TilePassScratch {
        feature_meta: Vec::new(),
        protected,
        has_dem_source: crate::terrain::source::dem_source(plan.style).is_some(),
        upload_started: instant::Instant::now(),
        time_budget,
        uploaded_any_bucket: false,
        any_deferred: false,
        needs_refetch: false,
        stamp,
        state: &mut state,
    };
    for &coords in &plan.source_tiles {
        upload_tile_buckets(buffer_pool, queue, tiles, &plan, coords, &mut pass);
    }
    (pass.any_deferred, pass.needs_refetch)
}

fn all_tiles_uploaded(tiles: &Tiles, source_tiles: &[WorldTileCoords], stamp: (u64, u64)) -> bool {
    source_tiles.iter().all(|&coords| {
        tiles
            .query::<(&VectorLayerBucketComponent, &LayerReplacements)>(coords)
            .is_some_and(|(layers, pending)| {
                layers.done && pending.vector.is_empty() && layers.uploaded_stamp == Some(stamp)
            })
    })
}

fn upload_tile_buckets(
    buffer_pool: &mut VectorBufferPool,
    queue: &UploadQueue,
    tiles: &mut Tiles,
    plan: &TessellatedUploadPlan<'_>,
    coords: WorldTileCoords,
    pass: &mut TilePassScratch<'_, '_>,
) {
    ensure_content(tiles, coords);
    if tiles
        .query::<(&VectorLayerBucketComponent, &LayerReplacements)>(coords)
        .is_none_or(|(layers, pending)| {
            layers.done && pending.vector.is_empty() && layers.uploaded_stamp == Some(pass.stamp)
        })
    {
        return;
    }
    let mut tile_incomplete = false;
    let mut uploaded = false;
    let mut pending_cpu_releases: Vec<(String, Option<u64>, bool)> = Vec::new();
    for style_layer in plan.shared_style_layers {
        match process_tile_style_layer(buffer_pool, queue, tiles, plan, coords, style_layer, pass) {
            LayerStep::Continue => continue,
            LayerStep::Incomplete => tile_incomplete = true,
            LayerStep::Exhausted => {
                shared_fallback::flush_safe_cpu_releases(
                    tiles,
                    buffer_pool,
                    coords,
                    &pending_cpu_releases,
                );
                pass.any_deferred = true;
                tile_incomplete = true;
                break;
            }
            LayerStep::Commit {
                replacement,
                did_upload,
                release_cpu,
            } => {
                uploaded |= did_upload;
                pass.uploaded_any_bucket |= did_upload;
                if let Some(index) = replacement {
                    commit_pending_vector(tiles, coords, index);
                }
                if let Some(shared_key) = release_cpu {
                    let release_colors =
                        !crate::style::feature_state::layer_paint_reads_feature_state(style_layer)
                            && !paint::feature_paint_depends_on_zoom(style_layer);
                    pending_cpu_releases.push((style_layer.id.clone(), shared_key, release_colors));
                }
            }
        }
    }
    shared_fallback::flush_safe_cpu_releases(tiles, buffer_pool, coords, &pending_cpu_releases);
    if !tile_incomplete {
        let end_stamp = (plan.style.revision(), buffer_pool.eviction_generation());
        if let Some((layers, pending)) =
            tiles.query_mut::<(&mut VectorLayerBucketComponent, &LayerReplacements)>(coords)
        {
            if layers.done && pending.vector.is_empty() {
                layers.uploaded_stamp = Some(end_stamp);
            }
        }
    }
    if uploaded {
        pass.state.budget.count_geometry_tile();
    }
}

fn process_tile_style_layer(
    buffer_pool: &mut VectorBufferPool,
    queue: &UploadQueue,
    tiles: &Tiles,
    plan: &TessellatedUploadPlan<'_>,
    coords: WorldTileCoords,
    style_layer: &Arc<StyleLayer>,
    pass: &mut TilePassScratch<'_, '_>,
) -> LayerStep {
    let Some((layers, pending)) =
        tiles.query::<(&VectorLayerBucketComponent, &LayerReplacements)>(coords)
    else {
        return LayerStep::Continue;
    };
    let replacement = pending
        .vector
        .iter()
        .position(|layer| layer.style_layer_id == style_layer.id);
    let bucket = replacement
        .and_then(|index| pending.vector.get(index))
        .or_else(|| {
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
        return LayerStep::Continue;
    };
    if replacement.is_none() && buffer_pool.has_layer(coords, &style_layer.id) {
        return LayerStep::Continue;
    }
    if !bucket.buffer.has_drawable_geometry() {
        buffer_pool.remove_layer(coords, &style_layer.id);
        return LayerStep::Commit {
            replacement,
            did_upload: false,
            release_cpu: None,
        };
    }
    let fallback_vertices = shared_fallback::find_shared_fallback_vertices(
        bucket,
        layers,
        pending,
        replacement.is_some(),
    );
    if !bucket.buffer.has_cpu_indices()
        || shared_fallback::missing_shared_vertices(
            buffer_pool,
            coords,
            style_layer,
            bucket,
            fallback_vertices,
        )
    {
        if !style_layer.is_hidden() {
            pass.needs_refetch = true;
        }
        return LayerStep::Incomplete;
    }
    execute_bucket_upload(
        buffer_pool,
        queue,
        tiles,
        plan,
        (
            coords,
            style_layer,
            bucket,
            fallback_vertices,
            replacement,
            layers.overscaled_zoom,
        ),
        pass,
    )
}

type BucketUploadSpec<'a> = (
    WorldTileCoords,
    &'a Arc<StyleLayer>,
    &'a crate::vector::AvailableVectorLayerBucket,
    Option<&'a [crate::render::ShaderVertex]>,
    Option<usize>,
    u8,
);

fn execute_bucket_upload(
    buffer_pool: &mut VectorBufferPool,
    queue: &UploadQueue,
    tiles: &Tiles,
    plan: &TessellatedUploadPlan<'_>,
    (coords, style_layer, bucket, fallback_vertices, replacement, overscaled_zoom): BucketUploadSpec<
        '_,
    >,
    pass: &mut TilePassScratch<'_, '_>,
) -> LayerStep {
    let is_uncovered = plan.uncovered_sources.contains(&coords);
    let is_dynamic_circle = layer_is_dynamic_circle(plan.style, style_layer);
    let is_uniform = !is_dynamic_circle
        && paint::uniform_color(style_layer, f64::from(plan.paint.zoom)).is_some();
    let bytes = upload_size(style_layer, bucket, is_uniform, is_dynamic_circle);
    if pass
        .time_budget
        .is_some_and(|limit| pass.uploaded_any_bucket && pass.upload_started.elapsed() >= limit)
        || pass.state.budget.geometry_tiles_exhausted_for(is_uncovered)
        || !pass.state.budget.can_take_geometry(bytes, is_uncovered)
    {
        return LayerStep::Exhausted;
    }
    let release_cpu = (plan.allow_cpu_release
        && shared_fallback::can_release_cpu_geometry(style_layer, bucket, pass.has_dem_source))
    .then(|| bucket.buffer.shared_geometry_key());
    let paint_store = tiles.query::<&TileFeaturePaintComponent>(coords);
    let paint_entries = paint_store.and_then(|s| {
        if replacement.is_some() {
            s.pending.get(&style_layer.id).map(Vec::as_slice)
        } else {
            s.committed.get(&style_layer.id).map(Vec::as_slice)
        }
    });
    let outcome = upload_bucket(
        buffer_pool,
        queue,
        BucketUploadInput {
            style_layer,
            bucket,
            fallback_vertices,
            paint_entries,
            overscaled_zoom,
            is_dynamic_circle,
            spatial: plan.spatial,
            paint: plan.paint,
            protected: &pass.protected,
        },
        BucketUploadScratch {
            feature_metadata: &mut pass.feature_meta,
            dyn_points: pass.state.dyn_points,
        },
    );
    match outcome {
        UploadOutcome::Uploaded(uploaded_slot) => {
            if let (Some(slots), Some((slot, slot_meta))) =
                (pass.state.paint_slots.as_deref_mut(), uploaded_slot)
            {
                slots.record(slot, slot_meta);
            }
            pass.state.budget.take_geometry(bytes, is_uncovered);
            LayerStep::Commit {
                replacement,
                did_upload: true,
                release_cpu,
            }
        }
        UploadOutcome::PoolSaturated | UploadOutcome::Failed => LayerStep::Incomplete,
    }
}
