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
mod metadata;
pub(crate) mod paint;
pub(crate) mod sources;
pub(crate) mod style_refresh;

use bucket::{upload_bucket, UploadOutcome};
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
        );
    }
    let (spatial, changed) = super::structures::for_frame(world, style, &painted_tiles);
    world.resources.get_or_init_mut::<FrameUploads>();
    world.resources.get_or_init_mut::<DynPoints>();
    world
        .resources
        .get_or_init_mut::<paint::PaintRefreshScratch>();
    #[cfg(feature = "geojson")]
    crate::geojson::update::dynamic_points::ensure_seeded_for_style(world, style);
    style_refresh::apply_dirty_style(world, style, queue, paint_frame);
    let retain_loaded_tiles = world
        .resources
        .get::<crate::render::systems::retention_system::RetainLoadedTiles>()
        .is_some();
    let Some((Initialized(buffer_pool), budget, dyn_points, paint_scratch)) =
        world.resources.query_mut::<(
            &mut Eventually<VectorBufferPool>,
            &mut FrameUploads,
            &mut DynPoints,
            &mut paint::PaintRefreshScratch,
        )>()
    else {
        return Err(SystemError::Dependencies);
    };
    if refresh_paint {
        paint_scratch.sync_style(style);
        paint::refresh_layer_paint(
            buffer_pool,
            queue,
            &world.tiles,
            &painted_tiles,
            paint_frame,
            previous_paint,
            paint_scratch,
        );
    }
    if changed {
        super::structures::refresh_gpu(buffer_pool, queue, &spatial, &world.tiles);
    }
    let (deferred, needs_refetch) = if has_pending_uploads {
        let allow_cpu_release = (style.terrain.is_none() || cfg!(target_arch = "wasm32"))
            && !retain_loaded_tiles
            && settings.buffer_pools.vertices >= 1_000_000;
        upload_tessellated_layer_with_uncovered(
            buffer_pool,
            queue,
            &mut world.tiles,
            style,
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
    if deferred {
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

fn frame_paint(
    world: &mut crate::tcs::world::World,
    style: &Style,
    view_state: &crate::render::view_state::ViewState,
) -> (VectorPaintFrame, Option<VectorPaintFrame>) {
    let zoom = if style.terrain.is_some() {
        paint::stabilize_zoom(world, view_state.style_zoom().value()) as f32
    } else {
        (view_state.style_zoom().level() * 256.0).round() / 256.0
    };
    let bearing = ((view_state.camera().get_bearing().0.to_degrees() * 4.0).round() * 0.25)
        .to_radians() as f32;
    let light = style
        .light
        .as_ref()
        .map_or_else(
            || {
                crate::style::light::LightSpecification::default()
                    .extrusion_light(view_state.style_zoom().value(), f64::from(bearing))
            },
            |light| light.extrusion_light(view_state.style_zoom().value(), f64::from(bearing)),
        )
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
    upload_tessellated_layer_with_uncovered(
        buffer_pool,
        queue,
        tiles,
        style,
        (source_tiles, &empty),
        (spatial, paint),
        (budget, allow_cpu_release, dyn_points),
    )
}

fn upload_tessellated_layer_with_uncovered(
    buffer_pool: &mut VectorBufferPool,
    queue: &crate::render::upload_queue::UploadQueue,
    tiles: &mut Tiles,
    style: &Style,
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
        for style_layer in &style.layers {
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
                    let bytes = upload_size(style_layer, bucket, is_uniform, is_dynamic_circle);
                    if budget.geometry_tiles_exhausted_for(is_uncovered)
                        || !budget.can_take_geometry(bytes, is_uncovered)
                    {
                        Step::Exhausted
                    } else {
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
                    metadata::flush_safe_cpu_releases(
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
        metadata::flush_safe_cpu_releases(tiles, buffer_pool, coords, &pending_cpu_releases);
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
