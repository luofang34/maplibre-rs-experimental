use super::*;

struct VectorPoolSyncMaps {
    touched_by_layer: HashMap<String, HashSet<WorldTileCoords>>,
    paint_changed_by_layer: HashMap<String, HashSet<WorldTileCoords>>,
    uploaded_by_layer: HashMap<String, HashSet<WorldTileCoords>>,
}

struct GpuPaintUpdate {
    coords: WorldTileCoords,
    layer_id: String,
    feature_meta: Vec<FillShaderFeatureMetadata>,
    runs: Option<Vec<(u32, std::ops::Range<u32>)>>,
    bump_revision: bool,
}

pub(super) fn refresh_vector_pool(
    world: &mut World,
    style: &Style,
    queue: &UploadQueue,
    frame: VectorPaintFrame,
    sync_all_layers: bool,
    recolor_layers: &HashSet<String>,
    needs_worker_refresh: &mut bool,
) {
    let Some(maps) = collect_vector_pool_sync_maps(world, style, sync_all_layers, recolor_layers)
    else {
        return;
    };
    let all_tile_coords: Vec<WorldTileCoords> =
        world.tiles.tiles.values().map(|tile| tile.coords).collect();
    let mut gpu_updates = Vec::new();
    for style_layer in &style.layers {
        if !recolor_layers.contains(&style_layer.id) {
            continue;
        }
        #[cfg(feature = "geojson")]
        if crate::geojson::update::dynamic_points::uses_dynamic_circle_metadata(style, style_layer)
        {
            continue;
        }
        refresh_style_layer_buckets(
            world,
            style_layer,
            frame,
            &all_tile_coords,
            &maps,
            &mut gpu_updates,
            needs_worker_refresh,
        );
    }
    commit_vector_pool_updates(
        world,
        style,
        queue,
        frame,
        &maps.touched_by_layer,
        gpu_updates,
    );
}

fn collect_vector_pool_sync_maps(
    world: &mut World,
    style: &Style,
    sync_all_layers: bool,
    recolor_layers: &HashSet<String>,
) -> Option<VectorPoolSyncMaps> {
    let Some(Initialized(vector_pool)) = world.resources.get_mut::<Eventually<VectorBufferPool>>()
    else {
        return None;
    };
    let mut touched_by_layer: HashMap<String, HashSet<WorldTileCoords>> = HashMap::default();
    let mut paint_changed_by_layer: HashMap<String, HashSet<WorldTileCoords>> = HashMap::default();
    let mut uploaded_by_layer: HashMap<String, HashSet<WorldTileCoords>> = HashMap::default();
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
    Some(VectorPoolSyncMaps {
        touched_by_layer,
        paint_changed_by_layer,
        uploaded_by_layer,
    })
}

fn refresh_style_layer_buckets(
    world: &mut World,
    style_layer: &crate::style::layer::StyleLayer,
    frame: VectorPaintFrame,
    all_tile_coords: &[WorldTileCoords],
    maps: &VectorPoolSyncMaps,
    gpu_updates: &mut Vec<GpuPaintUpdate>,
    needs_worker_refresh: &mut bool,
) {
    let uniform = paint::uniform_color(style_layer, f64::from(frame.zoom));
    let fallback_color: [f32; 4] = style_layer
        .paint
        .as_ref()
        .and_then(LayerPaint::get_color)
        .map(Into::into)
        .unwrap_or([0.0, 0.0, 0.0, 1.0]);
    let mut candidate_coords: HashSet<WorldTileCoords> = all_tile_coords.iter().copied().collect();
    if let Some(uploaded) = maps.uploaded_by_layer.get(&style_layer.id) {
        candidate_coords.extend(uploaded.iter().copied());
    }
    for coords in candidate_coords {
        refresh_tile_layer_bucket(
            world,
            style_layer,
            coords,
            (uniform, fallback_color),
            maps,
            gpu_updates,
            needs_worker_refresh,
        );
    }
}

fn refresh_tile_layer_bucket(
    world: &mut World,
    style_layer: &crate::style::layer::StyleLayer,
    coords: WorldTileCoords,
    (uniform, fallback_color): (Option<[f32; 4]>, [f32; 4]),
    maps: &VectorPoolSyncMaps,
    gpu_updates: &mut Vec<GpuPaintUpdate>,
    needs_worker_refresh: &mut bool,
) {
    let is_uploaded = maps
        .uploaded_by_layer
        .get(&style_layer.id)
        .is_some_and(|set| set.contains(&coords));
    let is_layer_touched = maps
        .touched_by_layer
        .get(&style_layer.id)
        .is_some_and(|set| set.contains(&coords));
    let is_paint_changed = maps
        .paint_changed_by_layer
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
            let _recolored = recolor_cpu_bucket(
                style_layer,
                bucket,
                pending_entries.as_deref(),
                uniform,
                style_zoom,
                needs_worker_refresh,
            );
        }
    }
    let handled = refresh_committed_cpu_bucket(
        world,
        style_layer,
        coords,
        committed_entries.as_deref(),
        (uniform, fallback_color, style_zoom),
        (is_uploaded, is_paint_changed, is_layer_touched),
        (gpu_updates, needs_worker_refresh),
    );
    if !handled && is_uploaded && is_paint_changed {
        if let Some(color) = uniform {
            push_uniform_only_update(
                world,
                style_layer,
                coords,
                color,
                is_layer_touched,
                gpu_updates,
            );
        }
    }
}

fn refresh_committed_cpu_bucket(
    world: &mut World,
    style_layer: &crate::style::layer::StyleLayer,
    coords: WorldTileCoords,
    committed_entries: Option<&[crate::vector::feature_paint::BucketFeatureEntry]>,
    (uniform, fallback_color, style_zoom): (Option<[f32; 4]>, [f32; 4], f64),
    (is_uploaded, is_paint_changed, is_layer_touched): (bool, bool, bool),
    (gpu_updates, needs_worker_refresh): (&mut Vec<GpuPaintUpdate>, &mut bool),
) -> bool {
    let entry_info = is_uploaded
        .then(|| entry_uniform_info(world, coords, &style_layer.id))
        .flatten();
    let Some(buckets) = world
        .tiles
        .query_mut::<&mut VectorLayerBucketComponent>(coords)
    else {
        return false;
    };
    let Some(bucket) = buckets.layers.iter_mut().find_map(|b| match b {
        VectorLayerBucket::AvailableLayer(avail) if avail.style_layer_id == style_layer.id => {
            Some(avail)
        }
        _ => None,
    }) else {
        return false;
    };
    if bucket.feature_indices.is_empty() && bucket.buffer.usable_indices > 0 {
        if uniform.is_none() {
            *needs_worker_refresh = true;
        }
        return false;
    }
    let (ok, colors_changed) = recolor_cpu_bucket(
        style_layer,
        bucket,
        committed_entries,
        uniform,
        style_zoom,
        needs_worker_refresh,
    );
    if !ok || !is_uploaded || (!colors_changed && !is_paint_changed) {
        return true;
    }
    let Some((entry_is_uniform, allocated_count)) = entry_info else {
        return true;
    };
    if entry_is_uniform && uniform.is_none() {
        *needs_worker_refresh = true;
        return true;
    }
    let metadata = if let Some(color) = uniform {
        vec![FillShaderFeatureMetadata::from_rgba(color); allocated_count]
    } else {
        let built = feature_metadata(
            style_layer,
            &bucket.feature_indices,
            &bucket.feature_colors,
            fallback_color,
        );
        if built.len() != allocated_count {
            *needs_worker_refresh = true;
            return true;
        }
        built
    };
    let has_per_feature_pattern = style_layer
        .paint
        .as_ref()
        .and_then(per_feature_pattern)
        .is_some();
    if has_per_feature_pattern
        && bucket.buffer.usable_indices > 0
        && bucket.buffer.buffer.indices.is_empty()
    {
        *needs_worker_refresh = true;
        return true;
    }
    let runs = has_per_feature_pattern.then(|| {
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
    true
}

fn entry_uniform_info(
    world: &World,
    coords: WorldTileCoords,
    layer_id: &str,
) -> Option<(bool, usize)> {
    let Some(Initialized(pool)) = world.resources.get::<Eventually<VectorBufferPool>>() else {
        return None;
    };
    pool.index()
        .get_layers(coords)
        .and_then(|l| l.iter().find(|e| e.style_layer.id == layer_id))
        .map(|e| {
            let count = (e.feature_metadata_buffer_range().end
                - e.feature_metadata_buffer_range().start) as usize
                / size_of::<FillShaderFeatureMetadata>();
            (e.is_uniform_feature_metadata(), count)
        })
}

fn push_uniform_only_update(
    world: &World,
    style_layer: &crate::style::layer::StyleLayer,
    coords: WorldTileCoords,
    color: [f32; 4],
    is_layer_touched: bool,
    gpu_updates: &mut Vec<GpuPaintUpdate>,
) {
    if let Some((_, count)) = entry_uniform_info(world, coords, &style_layer.id) {
        gpu_updates.push(GpuPaintUpdate {
            coords,
            layer_id: style_layer.id.clone(),
            feature_meta: vec![FillShaderFeatureMetadata::from_rgba(color); count],
            runs: None,
            bump_revision: !is_layer_touched,
        });
    }
}

fn commit_vector_pool_updates(
    world: &mut World,
    style: &Style,
    queue: &UploadQueue,
    frame: VectorPaintFrame,
    touched_by_layer: &HashMap<String, HashSet<WorldTileCoords>>,
    gpu_updates: Vec<GpuPaintUpdate>,
) {
    let evaluated_zooms: Vec<f32> = gpu_updates
        .iter()
        .map(|u| {
            let zoom = world
                .tiles
                .query::<&VectorLayerBucketComponent>(u.coords)
                .map_or(u8::from(u.coords.z), |b| {
                    b.overscaled_zoom.max(u8::from(u.coords.z))
                });
            f32::from(zoom)
        })
        .collect();
    let Some(Initialized(vector_pool)) = world.resources.get_mut::<Eventually<VectorBufferPool>>()
    else {
        return;
    };
    let mut updated_layer_slots: rustc_hash::FxHashSet<(u16, u64)> =
        rustc_hash::FxHashSet::default();
    let mut opaque_updates: Vec<(WorldTileCoords, crate::style::StyleLayerId, bool)> = Vec::new();
    for style_layer in &style.layers {
        let Some(coords_set) = touched_by_layer.get(&style_layer.id) else {
            continue;
        };
        let is_uniform_fill = style_layer.type_ == "fill"
            && crate::vector::upload_system::metadata::uses_layer_metadata_uniform_color(
                style_layer,
            )
            && !matches!(
                style_layer.paint.as_ref(),
                Some(LayerPaint::Fill(f)) if f.fill_pattern.is_some()
            )
            && paint::uniform_color(style_layer, f64::from(frame.zoom)).is_some();
        for &coords in coords_set {
            let layer_meta = metadata_for_layer(style_layer, coords, frame);
            if is_uniform_fill {
                opaque_updates.push((
                    coords,
                    style_layer.layer_id(),
                    layer_meta.uniform_color[3] >= 1.0,
                ));
            }
            if let Some(entry) = vector_pool
                .index()
                .get_layers(coords)
                .and_then(|layers| layers.iter().find(|e| e.style_layer.id == style_layer.id))
            {
                if updated_layer_slots
                    .insert((entry.page(), entry.layer_metadata_buffer_range().start))
                {
                    vector_pool.update_layer_metadata(queue, entry, layer_meta);
                }
            }
        }
    }
    for (coords, layer_id, opaque) in opaque_updates {
        vector_pool.set_layer_opaque_color_by_id(coords, layer_id, opaque);
    }
    apply_gpu_paint_updates(vector_pool, queue, gpu_updates, &evaluated_zooms);
}

fn apply_gpu_paint_updates(
    vector_pool: &mut VectorBufferPool,
    queue: &UploadQueue,
    gpu_updates: Vec<GpuPaintUpdate>,
    evaluated_zooms: &[f32],
) {
    for (
        GpuPaintUpdate {
            coords,
            layer_id,
            feature_meta,
            runs,
            bump_revision,
        },
        &evaluated_zoom,
    ) in gpu_updates.into_iter().zip(evaluated_zooms)
    {
        if let Some(entry) = vector_pool.index().get_layers(coords).and_then(|layers| {
            layers
                .iter()
                .find(|layers_entry| layers_entry.style_layer.id == layer_id)
        }) {
            vector_pool.update_feature_metadata(queue, entry, &feature_meta);
        }
        vector_pool.set_layer_evaluated_zoom(coords, &layer_id, evaluated_zoom);
        vector_pool.set_pattern_runs(coords, &layer_id, runs);
        if bump_revision {
            vector_pool.bump_layer_paint_revision(coords, &layer_id);
        }
    }
}
