//! Uploads data to the GPU which is needed for rendering.

use crate::{
    context::MapContext,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        memory_budget::FrameUploads,
        Renderer,
    },
    style::Style,
    tcs::{
        system::{SystemError, SystemResult},
        tiles::Tiles,
    },
    vector::{VectorBufferPool, VectorLayerBucket, VectorLayerBucketComponent},
};

mod bucket;
mod frame_state;
mod metadata;
pub(crate) mod paint;
mod shared_fallback;
pub(crate) mod sources;
pub(crate) mod style_refresh;

use bucket::{upload_bucket, SharedStyleLayers, UploadOutcome};
pub(crate) use frame_state::drape_paint_zoom;
use frame_state::{frame_paint, VectorPaintFrame};
use metadata::upload_size;
#[cfg(test)]
use metadata::{metadata_for_layer, pattern_runs, sort_runs};

#[cfg(feature = "geojson")]
type DynPoints = crate::geojson::update::dynamic_points::DynamicPointGeoJson;
#[cfg(not(feature = "geojson"))]
#[derive(Default)]
struct DynPoints;

#[inline]
fn layer_is_dynamic_circle(style: &Style, style_layer: &crate::style::layer::StyleLayer) -> bool {
    #[cfg(feature = "geojson")]
    {
        crate::geojson::update::dynamic_points::uses_dynamic_circle_metadata(style, style_layer)
    }
    #[cfg(not(feature = "geojson"))]
    {
        let _ = (style, style_layer);
        false
    }
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
        super::request_system::clear_upload_permits(world);
        return Ok(());
    }
    let (paint_frame, previous_paint) = frame_paint(world, style, view_state);
    let refresh_paint = previous_paint != Some(paint_frame);
    let (source_tiles, painted_tiles, uncovered_sources) =
        sources::frame_sources_and_painted(world, style, view_state)?;
    let has_pending_uploads = {
        let stamp = match world.resources.get::<Eventually<VectorBufferPool>>() {
            Some(Initialized(pool)) => (style.revision(), pool.eviction_generation()),
            _ => return Err(SystemError::Dependencies),
        };
        !source_tiles.iter().all(|&coords| {
            world
                .tiles
                .query::<(
                    &VectorLayerBucketComponent,
                    &super::content::LayerReplacements,
                )>(coords)
                .is_some_and(|(layers, pending)| {
                    layers.done && pending.vector.is_empty() && layers.uploaded_stamp == Some(stamp)
                })
        })
    };
    let permit_source_set =
        super::request_system::snapshot_permit_sources(world, has_pending_uploads, &source_tiles);
    let terrain_booting = style.terrain.is_some()
        && world
            .resources
            .get::<crate::terrain::request_system::DrapeRequests>()
            .is_none_or(|requests| requests.0.is_empty());
    if has_pending_uploads && !terrain_booting {
        crate::render::systems::retention_system::evict_before_upload(
            world,
            style,
            view_state,
            queue,
            settings.tight_tile_requests,
            &painted_tiles,
        );
    }
    let (spatial, changed) = super::structures::for_frame(world, style, &painted_tiles);
    world.resources.get_or_init_mut::<FrameUploads>();
    world.resources.get_or_init_mut::<DynPoints>();
    world
        .resources
        .get_or_init_mut::<paint::PaintRefreshScratch>();
    world.resources.get_or_init_mut::<SharedStyleLayers>();
    #[cfg(feature = "geojson")]
    crate::geojson::update::dynamic_points::ensure_seeded_for_style(world, style);
    style_refresh::apply_dirty_style(world, style, queue, paint_frame);
    let retain_loaded_tiles = world
        .resources
        .get::<crate::render::systems::retention_system::RetainLoadedTiles>()
        .is_some();
    let Some((Initialized(buffer_pool), budget, dyn_points, paint_scratch, shared_layers)) =
        world.resources.query_mut::<(
            &mut Eventually<VectorBufferPool>,
            &mut FrameUploads,
            &mut DynPoints,
            &mut paint::PaintRefreshScratch,
            &mut SharedStyleLayers,
        )>()
    else {
        return Err(SystemError::Dependencies);
    };
    let moving = budget.is_moving();
    if refresh_paint || (!moving && paint_scratch.needs_settle()) {
        paint_scratch.sync_style(style);
        paint::refresh_layer_paint(
            buffer_pool,
            queue,
            &world.tiles,
            &painted_tiles,
            paint_frame,
            previous_paint,
            moving,
            paint_scratch,
        );
    }
    let paint_settle_pending = paint_scratch.needs_settle();
    if changed {
        super::structures::refresh_gpu(buffer_pool, queue, &spatial, &world.tiles);
    }
    let (deferred, needs_refetch) = if has_pending_uploads {
        let allow_cpu_release = (style.terrain.is_none() || cfg!(target_arch = "wasm32"))
            && !retain_loaded_tiles
            && settings.buffer_pools.vertices >= 1_000_000;
        let shared_style_layers = shared_layers.sync(style);
        upload_tessellated_layer_with_uncovered(
            buffer_pool,
            queue,
            &mut world.tiles,
            style,
            shared_style_layers,
            (source_tiles, &uncovered_sources),
            (&spatial, paint_frame),
            (budget, allow_cpu_release, dyn_points),
        )
    } else {
        (false, false)
    };
    #[cfg(feature = "geojson")]
    crate::geojson::update::dynamic_points::refresh_dynamic_point_layers(
        world,
        style,
        queue,
        &painted_tiles,
    );
    if deferred || paint_settle_pending {
        crate::render::frame_signals::keep_animating(world);
    }
    super::request_system::reconcile_upload_permits(
        world,
        deferred,
        style.revision(),
        permit_source_set.as_ref(),
    );
    if needs_refetch {
        crate::render::frame_signals::mark_dirty(world);
    }
    Ok(())
}

#[cfg(test)]
#[allow(dead_code)]
fn upload_tessellated_layer(
    buffer_pool: &mut VectorBufferPool,
    queue: &crate::render::upload_queue::UploadQueue,
    tiles: &mut Tiles,
    style: &Style,
    source_tiles: Vec<crate::coords::WorldTileCoords>,
    (spatial, paint): (&[super::structures::SpatialBuffer], VectorPaintFrame),
    (budget, allow_cpu_release, dyn_points): (&mut FrameUploads, bool, &mut DynPoints),
) -> (bool, bool) {
    let empty = rustc_hash::FxHashSet::default();
    let shared: Vec<_> = style
        .layers
        .iter()
        .cloned()
        .map(std::sync::Arc::new)
        .collect();
    upload_tessellated_layer_with_uncovered(
        buffer_pool,
        queue,
        tiles,
        style,
        &shared,
        (source_tiles, &empty),
        (spatial, paint),
        (budget, allow_cpu_release, dyn_points),
    )
}

#[allow(clippy::too_many_arguments)]
fn upload_tessellated_layer_with_uncovered(
    buffer_pool: &mut VectorBufferPool,
    queue: &crate::render::upload_queue::UploadQueue,
    tiles: &mut Tiles,
    style: &Style,
    shared_style_layers: &[std::sync::Arc<crate::style::layer::StyleLayer>],
    (source_tiles, uncovered_sources): (
        Vec<crate::coords::WorldTileCoords>,
        &rustc_hash::FxHashSet<crate::coords::WorldTileCoords>,
    ),
    (spatial, paint): (&[super::structures::SpatialBuffer], VectorPaintFrame),
    (budget, allow_cpu_release, dyn_points): (&mut FrameUploads, bool, &mut DynPoints),
) -> (bool, bool) {
    let stamp = (style.revision(), buffer_pool.eviction_generation());
    if source_tiles.iter().all(|&coords| {
        tiles
            .query::<(
                &VectorLayerBucketComponent,
                &super::content::LayerReplacements,
            )>(coords)
            .is_some_and(|(layers, pending)| {
                layers.done && pending.vector.is_empty() && layers.uploaded_stamp == Some(stamp)
            })
    }) {
        return (false, false);
    }
    let has_dem_source = crate::terrain::source::dem_source(style).is_some();
    let mut feature_meta_scratch = Vec::new();
    let mut needs_refetch = false;
    let mut any_deferred = false;
    let upload_started = instant::Instant::now();
    let mut uploaded_any_bucket = false;
    let time_budget = if cfg!(target_arch = "wasm32") && budget.is_post_boot() {
        Some(if budget.is_moving() {
            std::time::Duration::from_micros(700)
        } else {
            std::time::Duration::from_micros(800)
        })
    } else {
        None
    };
    let protected: rustc_hash::FxHashSet<_> = source_tiles.iter().copied().collect();
    for coords in source_tiles {
        let is_uncovered = uncovered_sources.contains(&coords);
        super::content::ensure(tiles, coords);
        let Some((layers, pending)) = tiles.query::<(
            &VectorLayerBucketComponent,
            &super::content::LayerReplacements,
        )>(coords) else {
            continue;
        };
        if layers.done && pending.vector.is_empty() && layers.uploaded_stamp == Some(stamp) {
            continue;
        }
        let mut tile_incomplete = false;
        let mut uploaded = false;
        let mut pending_cpu_releases: Vec<(String, Option<u64>, bool)> = Vec::new();
        for style_layer in shared_style_layers {
            enum Step {
                Skip,
                Exhausted,
                Saturated,
                Commit {
                    replacement: Option<usize>,
                    did_upload: bool,
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
                let is_dynamic_circle = layer_is_dynamic_circle(style, style_layer);
                let is_uniform = !is_dynamic_circle
                    && paint::uniform_color(style_layer, f64::from(paint.zoom)).is_some();
                if !bucket.buffer.has_drawable_geometry() {
                    buffer_pool.remove_layer(coords, &style_layer.id);
                    Step::Commit {
                        replacement,
                        did_upload: false,
                        release_cpu: None,
                    }
                } else if !bucket.buffer.has_cpu_indices() {
                    if !style_layer.is_hidden() {
                        needs_refetch = true;
                    }
                    tile_incomplete = true;
                    continue;
                } else {
                    let fallback_vertices = shared_fallback::find_shared_fallback_vertices(
                        bucket,
                        layers,
                        pending,
                        replacement.is_some(),
                    );
                    if shared_fallback::missing_shared_vertices(
                        buffer_pool,
                        coords,
                        style_layer,
                        bucket,
                        fallback_vertices,
                    ) {
                        if !style_layer.is_hidden() {
                            needs_refetch = true;
                        }
                        tile_incomplete = true;
                        continue;
                    }
                    let bytes = upload_size(style_layer, bucket, is_uniform, is_dynamic_circle);
                    if time_budget.is_some_and(|limit| {
                        uploaded_any_bucket && upload_started.elapsed() >= limit
                    }) || budget.geometry_tiles_exhausted_for(is_uncovered)
                        || !budget.can_take_geometry(bytes, is_uncovered)
                    {
                        Step::Exhausted
                    } else {
                        let release_cpu = (allow_cpu_release
                            && shared_fallback::can_release_cpu_geometry(
                                style_layer,
                                bucket,
                                has_dem_source,
                            ))
                        .then(|| bucket.buffer.shared_geometry_key());
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
                            (paint_entries, layers.overscaled_zoom, is_dynamic_circle),
                            (spatial, paint),
                            (&protected, &mut feature_meta_scratch, dyn_points),
                        ) {
                            UploadOutcome::Uploaded => {
                                budget.take_geometry(bytes, is_uncovered);
                                Step::Commit {
                                    replacement,
                                    did_upload: true,
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
                Step::Skip | Step::Saturated => {
                    tile_incomplete = true;
                    continue;
                }
                Step::Exhausted => {
                    shared_fallback::flush_safe_cpu_releases(
                        tiles,
                        buffer_pool,
                        coords,
                        &pending_cpu_releases,
                    );
                    any_deferred = true;
                    tile_incomplete = true;
                    break;
                }
                Step::Commit {
                    replacement,
                    did_upload,
                    release_cpu,
                } => {
                    uploaded |= did_upload;
                    uploaded_any_bucket |= did_upload;
                    if let Some(index) = replacement {
                        super::content::commit_pending_vector(tiles, coords, index);
                    }
                    if let Some(shared_key) = release_cpu {
                        let release_colors =
                            !crate::style::feature_state::layer_paint_reads_feature_state(
                                style_layer,
                            ) && !paint::feature_paint_depends_on_zoom(style_layer);
                        pending_cpu_releases.push((
                            style_layer.id.clone(),
                            shared_key,
                            release_colors,
                        ));
                    }
                }
            }
        }
        shared_fallback::flush_safe_cpu_releases(tiles, buffer_pool, coords, &pending_cpu_releases);
        if !tile_incomplete {
            let end_stamp = (style.revision(), buffer_pool.eviction_generation());
            if let Some((layers, pending)) = tiles.query_mut::<(
                &mut VectorLayerBucketComponent,
                &super::content::LayerReplacements,
            )>(coords)
            {
                if layers.done && pending.vector.is_empty() {
                    layers.uploaded_stamp = Some(end_stamp);
                }
            }
        }
        if uploaded {
            budget.count_geometry_tile();
        }
    }
    (any_deferred, needs_refetch)
}

#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "headless", not(target_arch = "wasm32")))]
#[path = "upload_system/retry/tests.rs"]
mod retry_tests;
