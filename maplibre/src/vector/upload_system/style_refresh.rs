//! In-place GPU refresh for dynamic style layer order, visibility, paint, and `feature-state`.

mod feature_state;

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    sync::Arc,
};

use rustc_hash::FxHashMap;

use super::{
    metadata::{feature_metadata, metadata_for_layer},
    paint, pattern_runs, VectorPaintFrame,
};
use crate::{
    coords::WorldTileCoords,
    io::tile_retry::{self, RequestKind},
    render::{
        eventually::{Eventually, Eventually::Initialized},
        shaders::{FillShaderFeatureMetadata, ShaderLayerMetadata},
        upload_queue::UploadQueue,
    },
    sdf::SymbolBufferPool,
    style::{
        feature_state::{layer_geometry_reads_feature_state, layer_paint_reads_feature_state},
        layer::{LayerPaint, StyleLayer},
        Style,
    },
    tcs::world::World,
    vector::{
        content::LayerReplacements,
        feature_paint::{
            evaluate_bucket_colors, BucketEntryKind, BucketFeatureEntry, TileFeaturePaintComponent,
        },
        pattern::per_feature_pattern,
        AvailableVectorLayerBucket, VectorBufferPool, VectorLayerBucket,
        VectorLayerBucketComponent,
    },
};

/// Queued in-place style and `feature-state` mutations awaiting the next GPU upload pass.
#[derive(Default, Debug)]
pub(crate) struct DirtyVectorStyle {
    /// Sync `StyleLayer` metadata (`index`, zoom range, visibility, uniforms) on GPU pools.
    pub sync_layers: bool,
    /// Style layer IDs whose per-feature colors/opacity/outline may have changed.
    pub paint_layers: BTreeSet<String>,
    /// Style source names whose runtime `feature-state` changed.
    pub feature_state_sources: BTreeSet<String>,
}

impl DirtyVectorStyle {
    /// Queues synchronization of layer order, zoom bounds, visibility, and paint for `layers`.
    pub fn mark_style_edit(world: &mut World, paint_layers: impl IntoIterator<Item = String>) {
        let dirty = world.resources.get_or_init_mut::<Self>();
        dirty.sync_layers = true;
        dirty.paint_layers.extend(paint_layers);
    }

    /// Queues an in-place `feature-state` update for `source`.
    pub fn mark_feature_state(world: &mut World, source: impl Into<String>) {
        let dirty = world.resources.get_or_init_mut::<Self>();
        dirty.feature_state_sources.insert(source.into());
    }
}

pub(super) fn apply_dirty_style(
    world: &mut World,
    style: &Style,
    queue: &UploadQueue,
    frame: VectorPaintFrame,
) {
    let Some(dirty) = world.resources.remove::<DirtyVectorStyle>() else {
        return;
    };
    let mut needs_worker_refresh = false;
    let mut recolor_layers = dirty.paint_layers;

    if !dirty.feature_state_sources.is_empty() {
        let mut memo = FxHashMap::default();
        for source in &dirty.feature_state_sources {
            world
                .tiles
                .geometry_index
                .sync_source_feature_state_with_memo(
                    source,
                    style.source_promote_id(source),
                    style.feature_state.source(source),
                    &mut memo,
                );
        }
        for layer in &style.layers {
            let Some(source) = layer.source.as_deref() else {
                continue;
            };
            if !dirty.feature_state_sources.contains(source) {
                continue;
            };
            if layer_geometry_reads_feature_state(layer) {
                needs_worker_refresh = true;
            }
            if layer.type_ != "symbol" && layer_paint_reads_feature_state(layer) {
                recolor_layers.insert(layer.id.clone());
            }
        }
        feature_state::sync_tile_feature_states(
            world,
            style,
            &dirty.feature_state_sources,
            &mut memo,
            &mut needs_worker_refresh,
        );
    }

    if dirty.sync_layers {
        sync_symbol_pool_layers(world, style, queue);
    }

    if dirty.sync_layers || !recolor_layers.is_empty() {
        refresh_vector_pool(
            world,
            style,
            queue,
            frame,
            dirty.sync_layers,
            &recolor_layers,
            &mut needs_worker_refresh,
        );
    }

    if needs_worker_refresh {
        tile_retry::refresh(world, RequestKind::Vector);
    }
}

fn sync_symbol_pool_layers(world: &mut World, style: &Style, queue: &UploadQueue) {
    let Some(Initialized(symbol_pool)) = world.resources.get_mut::<Eventually<SymbolBufferPool>>()
    else {
        return;
    };
    for style_layer in &style.layers {
        if style_layer.type_ != "symbol" {
            continue;
        }
        let touched = symbol_pool.sync_style_layer(style_layer);
        let metadata = ShaderLayerMetadata::new(style_layer.index as f32, 0.0, [0.0; 2]);
        for coords in touched {
            if let Some(entry) = symbol_pool
                .index()
                .get_layers(coords)
                .and_then(|layers| layers.iter().find(|e| e.style_layer.id == style_layer.id))
            {
                symbol_pool.update_layer_metadata(queue, entry, metadata);
            }
        }
    }
}

fn refresh_vector_pool(
    world: &mut World,
    style: &Style,
    queue: &UploadQueue,
    frame: VectorPaintFrame,
    sync_all_layers: bool,
    recolor_layers: &BTreeSet<String>,
    needs_worker_refresh: &mut bool,
) {
    let mut touched_by_layer: HashMap<String, HashSet<WorldTileCoords>> = HashMap::new();
    let mut paint_changed_by_layer: HashMap<String, HashSet<WorldTileCoords>> = HashMap::new();
    let mut uploaded_by_layer: HashMap<String, HashSet<WorldTileCoords>> = HashMap::new();
    {
        let Some(Initialized(vector_pool)) =
            world.resources.get_mut::<Eventually<VectorBufferPool>>()
        else {
            return;
        };
        if sync_all_layers {
            for style_layer in &style.layers {
                let paint_coords: HashSet<WorldTileCoords> = vector_pool
                    .index()
                    .iter()
                    .flatten()
                    .filter(|e| {
                        e.style_layer.id == style_layer.id && !e.style_layer.same_paint(style_layer)
                    })
                    .map(|e| e.coords)
                    .collect();
                if !paint_coords.is_empty() {
                    paint_changed_by_layer.insert(style_layer.id.clone(), paint_coords);
                }
                let coords: HashSet<WorldTileCoords> = vector_pool
                    .sync_style_layer(style_layer)
                    .into_iter()
                    .collect();
                if !coords.is_empty() {
                    touched_by_layer.insert(style_layer.id.clone(), coords);
                }
            }
        }
        for layer_id in recolor_layers {
            let coords: HashSet<WorldTileCoords> = vector_pool
                .index()
                .iter()
                .flatten()
                .filter(|e| &e.style_layer.id == layer_id)
                .map(|e| e.coords)
                .collect();
            if !coords.is_empty() {
                uploaded_by_layer.insert(layer_id.clone(), coords);
            }
        }
    }

    struct GpuPaintUpdate {
        coords: WorldTileCoords,
        layer_id: String,
        feature_meta: Vec<FillShaderFeatureMetadata>,
        runs: Option<Vec<(u32, std::ops::Range<u32>)>>,
        bump_revision: bool,
    }

    let all_tile_coords: Vec<WorldTileCoords> =
        world.tiles.tiles.values().map(|tile| tile.coords).collect();
    let mut gpu_updates: Vec<GpuPaintUpdate> = Vec::new();

    for style_layer in &style.layers {
        if !recolor_layers.contains(&style_layer.id) {
            continue;
        }
        let uniform = paint::uniform_color(style_layer, f64::from(frame.zoom));
        let fallback_color: [f32; 4] = style_layer
            .paint
            .as_ref()
            .and_then(LayerPaint::get_color)
            .map(Into::into)
            .unwrap_or([0.0, 0.0, 0.0, 1.0]);
        let mut candidate_coords: BTreeSet<WorldTileCoords> =
            all_tile_coords.iter().copied().collect();
        if let Some(uploaded) = uploaded_by_layer.get(&style_layer.id) {
            candidate_coords.extend(uploaded.iter().copied());
        }

        for coords in candidate_coords {
            let is_uploaded = uploaded_by_layer
                .get(&style_layer.id)
                .is_some_and(|set| set.contains(&coords));
            let is_layer_touched = touched_by_layer
                .get(&style_layer.id)
                .is_some_and(|set| set.contains(&coords));
            let is_paint_changed = paint_changed_by_layer
                .get(&style_layer.id)
                .is_some_and(|set| set.contains(&coords));
            let (committed_entries, pending_entries) = world
                .tiles
                .query::<&TileFeaturePaintComponent>(coords)
                .map(|store| {
                    (
                        store.committed.get(&style_layer.id).cloned(),
                        store.pending.get(&style_layer.id).cloned(),
                    )
                })
                .unwrap_or((None, None));
            let overscaled_zoom = world
                .tiles
                .query::<&VectorLayerBucketComponent>(coords)
                .map_or(0, |b| b.overscaled_zoom);
            let style_zoom = f64::from(overscaled_zoom.max(u8::from(coords.z)));

            if let Some(pending) = world.tiles.query_mut::<&mut LayerReplacements>(coords) {
                if let Some(bucket) = pending
                    .vector
                    .iter_mut()
                    .find(|b| b.style_layer_id == style_layer.id)
                {
                    let _ = recolor_cpu_bucket(
                        style_layer,
                        bucket,
                        pending_entries.as_deref(),
                        uniform,
                        style_zoom,
                        needs_worker_refresh,
                    );
                }
            }

            let mut handled = false;
            if let Some(buckets) = world
                .tiles
                .query_mut::<&mut VectorLayerBucketComponent>(coords)
            {
                if let Some(bucket) = buckets.layers.iter_mut().find_map(|b| match b {
                    VectorLayerBucket::AvailableLayer(avail)
                        if avail.style_layer_id == style_layer.id =>
                    {
                        Some(avail)
                    }
                    _ => None,
                }) {
                    handled = true;
                    let (ok, colors_changed) = recolor_cpu_bucket(
                        style_layer,
                        bucket,
                        committed_entries.as_deref(),
                        uniform,
                        style_zoom,
                        needs_worker_refresh,
                    );
                    if !ok || !is_uploaded || (!colors_changed && !is_paint_changed) {
                        continue;
                    }
                    let metadata = if let Some(color) = uniform {
                        feature_metadata(&bucket.feature_indices, &[], color)
                    } else {
                        feature_metadata(
                            &bucket.feature_indices,
                            &bucket.feature_colors,
                            fallback_color,
                        )
                    };
                    let runs = style_layer
                        .paint
                        .as_ref()
                        .and_then(per_feature_pattern)
                        .map(|_| {
                            pattern_runs(
                                &bucket.feature_indices,
                                &bucket.feature_colors,
                                &bucket.buffer.buffer.indices,
                            )
                        });
                    gpu_updates.push(GpuPaintUpdate {
                        coords,
                        layer_id: style_layer.id.clone(),
                        feature_meta: metadata,
                        runs,
                        bump_revision: !is_layer_touched,
                    });
                }
            }
            if !handled && is_uploaded && is_paint_changed {
                if let Some(color) = uniform {
                    if let Some(Initialized(pool)) =
                        world.resources.get::<Eventually<VectorBufferPool>>()
                    {
                        if let Some(entry) = pool
                            .index()
                            .get_layers(coords)
                            .and_then(|l| l.iter().find(|e| e.style_layer.id == style_layer.id))
                        {
                            let count = (entry.feature_metadata_buffer_range().end
                                - entry.feature_metadata_buffer_range().start)
                                as usize
                                / size_of::<FillShaderFeatureMetadata>();
                            gpu_updates.push(GpuPaintUpdate {
                                coords,
                                layer_id: style_layer.id.clone(),
                                feature_meta: vec![FillShaderFeatureMetadata { color }; count],
                                runs: None,
                                bump_revision: !is_layer_touched,
                            });
                        }
                    }
                }
            }
        }
    }

    let Some(Initialized(vector_pool)) = world.resources.get_mut::<Eventually<VectorBufferPool>>()
    else {
        return;
    };
    for style_layer in &style.layers {
        let Some(coords_set) = touched_by_layer.get(&style_layer.id) else {
            continue;
        };
        for &coords in coords_set {
            let layer_meta = metadata_for_layer(style_layer, coords, frame);
            if let Some(entry) = vector_pool
                .index()
                .get_layers(coords)
                .and_then(|layers| layers.iter().find(|e| e.style_layer.id == style_layer.id))
            {
                vector_pool.update_layer_metadata(queue, entry, layer_meta);
            }
        }
    }
    for GpuPaintUpdate {
        coords,
        layer_id,
        feature_meta,
        runs,
        bump_revision,
    } in gpu_updates
    {
        if let Some(entry) = vector_pool
            .index()
            .get_layers(coords)
            .and_then(|layers| layers.iter().find(|e| e.style_layer.id == layer_id))
        {
            vector_pool.update_feature_metadata(queue, entry, &feature_meta);
        }
        vector_pool.set_pattern_runs(coords, &layer_id, runs);
        if bump_revision {
            vector_pool.bump_layer_paint_revision(coords, &layer_id);
        }
    }
}

fn recolor_cpu_bucket(
    style_layer: &StyleLayer,
    bucket: &mut AvailableVectorLayerBucket,
    entries: Option<&[BucketFeatureEntry]>,
    uniform: Option<[f32; 4]>,
    style_zoom: f64,
    needs_worker_refresh: &mut bool,
) -> (bool, bool) {
    if let Some(entries) = entries {
        let new_colors = evaluate_bucket_colors(style_layer, entries, style_zoom);
        let changed = new_colors != bucket.feature_colors;
        bucket.feature_colors = new_colors;
        (true, changed)
    } else if uniform.is_some() {
        (true, false)
    } else if layer_is_feature_constant(style_layer) {
        let single = [BucketFeatureEntry {
            raw_id: None,
            properties: Arc::default(),
            kind: BucketEntryKind::Feature,
        }];
        let const_color = evaluate_bucket_colors(style_layer, &single, style_zoom)[0];
        let new_colors = vec![const_color; bucket.feature_indices.len()];
        let changed = new_colors != bucket.feature_colors;
        bucket.feature_colors = new_colors;
        (true, changed)
    } else {
        if !bucket.buffer.buffer.indices.is_empty() {
            *needs_worker_refresh = true;
        }
        (false, false)
    }
}

fn layer_is_feature_constant(layer: &StyleLayer) -> bool {
    match layer.paint.as_ref() {
        Some(LayerPaint::Circle(c)) => {
            c.circle_color
                .as_ref()
                .is_none_or(|p| p.is_feature_constant())
                && c.circle_opacity
                    .as_ref()
                    .is_none_or(|p| p.is_feature_constant())
                && c.stroke_color_per_feature().is_none()
        }
        Some(LayerPaint::FillExtrusion(e)) => {
            e.fill_extrusion_color
                .as_ref()
                .is_none_or(|p| p.is_feature_constant())
                && e.fill_extrusion_opacity
                    .as_ref()
                    .is_none_or(|p| p.is_feature_constant())
        }
        _ => false,
    }
}
