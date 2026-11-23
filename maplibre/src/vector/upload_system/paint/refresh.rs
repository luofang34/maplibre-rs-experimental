use super::*;
use crate::{render::upload_queue::UploadQueue, vector::IndexEntry};

pub(in crate::vector::upload_system) struct PaintRefreshContext<'a> {
    pub tiles: &'a Tiles,
    pub source_tiles: &'a [WorldTileCoords],
    pub frame: VectorPaintFrame,
    pub previous: Option<VectorPaintFrame>,
    pub moving: bool,
    pub tiles_changed: bool,
}

struct PaintChangeFlags {
    zoom_changed: bool,
    bearing_changed: bool,
    light_changed: bool,
    settle_pass: bool,
    tiles_changed: bool,
    feature_zoom_threshold: f32,
}

impl PaintChangeFlags {
    fn from_context(ctx: &PaintRefreshContext<'_>, settle_pending: bool) -> Self {
        let zoom = ctx.frame.zoom;
        Self {
            zoom_changed: ctx.previous.is_none_or(|prev| prev.zoom != zoom),
            bearing_changed: ctx
                .previous
                .is_none_or(|prev| prev.bearing != ctx.frame.bearing),
            light_changed: ctx
                .previous
                .is_none_or(|prev| prev.light != ctx.frame.light),
            settle_pass: !ctx.moving && settle_pending,
            tiles_changed: ctx.tiles_changed,
            feature_zoom_threshold: feature_zoom_threshold(
                ctx.moving,
                ctx.previous.map_or(0.0, |prev| (zoom - prev.zoom).abs()),
            ),
        }
    }

    fn has_any_change(&self) -> bool {
        self.zoom_changed
            || self.bearing_changed
            || self.light_changed
            || self.settle_pass
            || self.tiles_changed
    }

    fn active_flags(&self) -> u8 {
        (u8::from(self.zoom_changed || self.tiles_changed) * (1 | 8 | 64))
            | (u8::from(self.bearing_changed || self.tiles_changed) * 2)
            | (u8::from(self.light_changed || self.tiles_changed) * 4)
            | (u8::from(self.settle_pass) * 8)
    }

    fn effective_frame_changed(&self, union: u8) -> bool {
        (self.zoom_changed && (union & (1 | 8 | 64)) != 0)
            || (self.bearing_changed && (union & 2) != 0)
            || (self.light_changed && (union & 4) != 0)
            || (self.settle_pass && (union & 8) != 0)
    }
}

#[inline]
fn normalized_frame(frame: VectorPaintFrame, union: u8) -> VectorPaintFrame {
    VectorPaintFrame {
        zoom: if (union & (1 | 8 | 64)) != 0 {
            frame.zoom
        } else {
            0.0
        },
        bearing: if (union & 2) != 0 { frame.bearing } else { 0.0 },
        light: if (union & 4) != 0 {
            frame.light
        } else {
            Default::default()
        },
    }
}

struct PaintEvalEnv<'a> {
    queue: &'a UploadQueue,
    ctx: &'a PaintRefreshContext<'a>,
    changes: &'a PaintChangeFlags,
    paint_started: instant::Instant,
}

fn tile_entries_stamp(layers: &std::collections::VecDeque<IndexEntry>) -> u64 {
    let mut acc = layers.len() as u64;
    for entry in layers {
        acc = acc.rotate_left(7)
            ^ (u64::from(entry.style_layer.index) << 32)
            ^ entry.layer_metadata_buffer_range().start
            ^ u64::from(entry.evaluated_zoom().map_or(u32::MAX, f32::to_bits));
    }
    acc
}

fn sync_tile_paint_stamp_cache(
    ctx: &PaintRefreshContext<'_>,
    frame_changed: bool,
    scratch: &mut PaintRefreshScratch,
) {
    if frame_changed {
        scratch.tile_paint_stamps.clear();
    } else if scratch.tile_paint_stamps.len() > ctx.source_tiles.len() + 64 {
        let source_set: FxHashSet<WorldTileCoords> = ctx.source_tiles.iter().copied().collect();
        scratch
            .tile_paint_stamps
            .retain(|c, _| source_set.contains(c));
    }
}

pub(in crate::vector::upload_system) fn refresh_layer_paint(
    buffer_pool: &mut VectorBufferPool,
    queue: &UploadQueue,
    ctx: PaintRefreshContext<'_>,
    scratch: &mut PaintRefreshScratch,
) {
    let changes = PaintChangeFlags::from_context(&ctx, scratch.needs_feature_zoom_settle);
    if !changes.has_any_change() {
        return;
    }
    let union = scratch.style_flags_union;
    let active_flags = changes.active_flags();
    if ctx.previous.is_some() && (union & active_flags) == 0 {
        if changes.settle_pass {
            scratch.needs_feature_zoom_settle = false;
        }
        return;
    }
    if changes.settle_pass {
        scratch.needs_feature_zoom_settle = false;
    }
    let effective_frame_changed = changes.effective_frame_changed(union);
    let stamp_frame = normalized_frame(ctx.frame, union);
    sync_tile_paint_stamp_cache(&ctx, effective_frame_changed, scratch);
    scratch.clear();
    let env = PaintEvalEnv {
        queue,
        ctx: &ctx,
        changes: &changes,
        paint_started: instant::Instant::now(),
    };
    let mut updated_any_feature_layer = false;

    for &coords in ctx.source_tiles {
        let Some(layers) = buffer_pool.index().get_layers(coords) else {
            continue;
        };
        let stamp = tile_entries_stamp(layers);
        if !effective_frame_changed
            && scratch.tile_paint_stamps.get(&coords) == Some(&(stamp_frame, stamp))
        {
            continue;
        }
        for entry in layers {
            let flags = *scratch
                .layer_flags
                .entry(entry.style_layer.index)
                .or_insert_with(|| layer_paint_flags(&entry.style_layer));
            if (flags & active_flags) == 0 {
                continue;
            }
            refresh_entry_layer_meta(buffer_pool, &env, coords, entry, flags, scratch);
            let skip_uniform = refresh_entry_feature_paint(
                buffer_pool,
                &env,
                (coords, flags),
                entry,
                &mut updated_any_feature_layer,
                scratch,
            );
            if !skip_uniform && changes.zoom_changed && (flags & 32) == 0 && (flags & 64) != 0 {
                refresh_entry_uniform_feature_color(buffer_pool, &env, coords, entry, scratch);
            }
        }
        scratch
            .tile_paint_stamps
            .insert(coords, (stamp_frame, stamp));
    }
    apply_deferred_paint_updates(buffer_pool, (ctx.frame, stamp_frame), scratch);
}

fn refresh_entry_layer_meta(
    buffer_pool: &VectorBufferPool,
    env: &PaintEvalEnv<'_>,
    coords: WorldTileCoords,
    entry: &IndexEntry,
    flags: u8,
    scratch: &mut PaintRefreshScratch,
) {
    let changes = env.changes;
    let meta_dirty = ((changes.zoom_changed || changes.tiles_changed) && (flags & 1) != 0)
        || ((changes.bearing_changed || changes.tiles_changed) && (flags & 2) != 0)
        || ((changes.light_changed || changes.tiles_changed) && (flags & 4) != 0);
    if !meta_dirty {
        return;
    }
    let key = (entry.style_layer.index, u8::from(coords.z));
    let (metadata, changed_vs_prev) = *scratch.meta_memo.entry(key).or_insert_with(|| {
        let metadata = metadata_for_layer(&entry.style_layer, coords, env.ctx.frame);
        let changed = match (env.ctx.previous, scratch.prev_meta.get(&key)) {
            (Some(prev_frame), Some((cached_frame, cached_meta)))
                if *cached_frame == prev_frame =>
            {
                bytemuck::bytes_of(cached_meta) != bytemuck::bytes_of(&metadata)
            }
            (Some(prev_frame), _) => {
                bytemuck::bytes_of(&metadata_for_layer(&entry.style_layer, coords, prev_frame))
                    != bytemuck::bytes_of(&metadata)
            }
            (None, _) => true,
        };
        scratch.prev_meta.insert(key, (env.ctx.frame, metadata));
        (metadata, changed)
    });
    let slot_range = entry.layer_metadata_buffer_range();
    let has_slot = slot_range.end > slot_range.start;
    let slot_key = (entry.page(), slot_range.start);
    let slot_stale = has_slot && scratch.slots.is_stale(slot_key, &metadata);
    if (changed_vs_prev || slot_stale) && has_slot && scratch.updated_layer_slots.insert(slot_key) {
        buffer_pool.update_layer_metadata(env.queue, entry, metadata);
        scratch.slots.record(slot_key, metadata);
    }
    if entry.style_layer.type_ == "fill"
        && (flags & 32) != 0
        && !is_patterned_fill(&entry.style_layer)
        && uniform_color(&entry.style_layer, f64::from(env.ctx.frame.zoom)).is_some()
    {
        let expected_opaque = metadata.uniform_color[3] >= 1.0;
        if changed_vs_prev || entry.is_opaque_color() != expected_opaque {
            scratch
                .opaque_updates
                .push((coords, entry.style_layer_id, expected_opaque));
        }
    }
}

fn is_patterned_fill(layer: &StyleLayer) -> bool {
    matches!(
        layer.paint.as_ref(),
        Some(LayerPaint::Fill(f)) if f.fill_pattern.is_some()
    )
}

fn refresh_entry_feature_paint(
    buffer_pool: &VectorBufferPool,
    env: &PaintEvalEnv<'_>,
    (coords, flags): (WorldTileCoords, u8),
    entry: &IndexEntry,
    updated_any_feature_layer: &mut bool,
    scratch: &mut PaintRefreshScratch,
) -> bool {
    let changes = env.changes;
    if !(changes.zoom_changed || changes.settle_pass || changes.tiles_changed)
        || entry.is_uniform_feature_metadata()
        || entry.is_dynamic_circle_metadata()
        || (flags & 8) == 0
    {
        return false;
    }
    let zoom = env.ctx.frame.zoom;
    let delta = entry
        .evaluated_zoom()
        .map(|evaluated| (zoom - evaluated).abs());
    if delta.is_some_and(|d| d < changes.feature_zoom_threshold) {
        if env.ctx.moving && delta.is_some_and(|d| d >= 1.0 / 32.0) {
            scratch.needs_feature_zoom_settle = true;
        }
        return false;
    }
    if let Some(evaluated) = entry.evaluated_zoom() {
        if (flags & 16) == 0
            && quantized_layer_opacity(&entry.style_layer, zoom)
                .zip(quantized_layer_opacity(&entry.style_layer, evaluated))
                .is_some_and(|(a, b)| a == b)
        {
            scratch
                .feature_zoom_only_updates
                .push((coords, entry.style_layer.id.clone()));
            return true;
        }
    }
    if cfg!(target_arch = "wasm32")
        && (env.ctx.moving || changes.settle_pass)
        && *updated_any_feature_layer
        && env.paint_started.elapsed() >= std::time::Duration::from_micros(400)
    {
        scratch.needs_feature_zoom_settle = true;
        return true;
    }
    update_feature_paint_from_tile(
        buffer_pool,
        env,
        coords,
        entry,
        updated_any_feature_layer,
        scratch,
    );
    false
}

fn update_feature_paint_from_tile(
    buffer_pool: &VectorBufferPool,
    env: &PaintEvalEnv<'_>,
    coords: WorldTileCoords,
    entry: &IndexEntry,
    updated_any_feature_layer: &mut bool,
    scratch: &mut PaintRefreshScratch,
) {
    let Some((paint_store, buckets)) = env
        .ctx
        .tiles
        .query::<(&TileFeaturePaintComponent, &VectorLayerBucketComponent)>(coords)
    else {
        return;
    };
    let entries = paint_store.committed.get(&entry.style_layer.id);
    let bucket = buckets.layers.iter().find_map(|layer| match layer {
        VectorLayerBucket::AvailableLayer(avail)
            if avail.style_layer_id == entry.style_layer.id =>
        {
            Some(avail)
        }
        _ => None,
    });
    let (Some(entries), Some(bucket)) = (entries, bucket) else {
        return;
    };
    if bucket.feature_indices.is_empty() {
        return;
    }
    let allocated_count = (entry.feature_metadata_buffer_range().end
        - entry.feature_metadata_buffer_range().start) as usize
        / size_of::<FillShaderFeatureMetadata>();
    let zoom = env.ctx.frame.zoom;
    let colors = evaluate_bucket_colors(&entry.style_layer, entries, f64::from(zoom));
    let fallback_color: [f32; 4] = entry
        .style_layer
        .paint
        .as_ref()
        .and_then(|paint| paint.color_at(f64::from(zoom)))
        .map(Into::into)
        .unwrap_or([0.0, 0.0, 0.0, 1.0]);
    feature_metadata_into(
        &mut scratch.feature_meta_scratch,
        &entry.style_layer,
        &bucket.feature_indices,
        &colors,
        fallback_color,
    );
    if scratch.feature_meta_scratch.len() != allocated_count {
        return;
    }
    buffer_pool.update_feature_metadata(env.queue, entry, &scratch.feature_meta_scratch);
    *updated_any_feature_layer = true;
    queue_pattern_and_opaque_updates(coords, entry, bucket, &colors, scratch);
}

fn queue_pattern_and_opaque_updates(
    coords: WorldTileCoords,
    entry: &IndexEntry,
    bucket: &crate::vector::AvailableVectorLayerBucket,
    colors: &[[f32; 4]],
    scratch: &mut PaintRefreshScratch,
) {
    let runs = entry
        .style_layer
        .paint
        .as_ref()
        .and_then(crate::vector::pattern::per_feature_pattern)
        .filter(|_| !bucket.buffer.buffer.indices.is_empty())
        .map(|_| {
            pattern_runs(
                &bucket.feature_indices,
                colors,
                &bucket.buffer.buffer.indices,
            )
        });
    scratch
        .feature_zoom_updates
        .push((coords, entry.style_layer.id.clone(), runs));
    if entry.style_layer.type_ == "fill" && !is_patterned_fill(&entry.style_layer) {
        scratch.opaque_updates.push((
            coords,
            entry.style_layer_id,
            !scratch.feature_meta_scratch.is_empty()
                && scratch
                    .feature_meta_scratch
                    .iter()
                    .all(|fm| fm.color[3] == 255),
        ));
    }
}

fn refresh_entry_uniform_feature_color(
    buffer_pool: &VectorBufferPool,
    env: &PaintEvalEnv<'_>,
    coords: WorldTileCoords,
    entry: &IndexEntry,
    scratch: &mut PaintRefreshScratch,
) {
    let zoom = env.ctx.frame.zoom;
    let changed_color = *scratch
        .color_memo
        .entry(entry.style_layer.index)
        .or_insert_with(|| {
            let color = uniform_color(&entry.style_layer, f64::from(zoom))?;
            let unchanged = env
                .ctx
                .previous
                .and_then(|prev| uniform_color(&entry.style_layer, f64::from(prev.zoom)))
                .is_some_and(|prev_color| same_quantized_rgba(prev_color, color));
            (!unchanged).then_some(color)
        });
    if let Some(color) = changed_color {
        buffer_pool.fill_feature_metadata(
            env.queue,
            entry,
            FillShaderFeatureMetadata::from_rgba(color),
        );
        if entry.style_layer.type_ == "fill" {
            scratch
                .opaque_updates
                .push((coords, entry.style_layer_id, color[3] >= 1.0));
        }
    }
}

fn apply_deferred_paint_updates(
    buffer_pool: &mut VectorBufferPool,
    (frame, stamp_frame): (VectorPaintFrame, VectorPaintFrame),
    scratch: &mut PaintRefreshScratch,
) {
    for (coords, layer_id, runs) in scratch.feature_zoom_updates.drain(..) {
        buffer_pool.set_layer_evaluated_zoom(coords, &layer_id, frame.zoom);
        if runs.is_some() {
            buffer_pool.set_pattern_runs(coords, &layer_id, runs);
        }
        buffer_pool.bump_layer_paint_revision(coords, &layer_id);
        if let Some(layers) = buffer_pool.index().get_layers(coords) {
            scratch
                .tile_paint_stamps
                .insert(coords, (stamp_frame, tile_entries_stamp(layers)));
        }
    }
    for (coords, layer_id) in scratch.feature_zoom_only_updates.drain(..) {
        buffer_pool.set_layer_evaluated_zoom(coords, &layer_id, frame.zoom);
        if let Some(layers) = buffer_pool.index().get_layers(coords) {
            scratch
                .tile_paint_stamps
                .insert(coords, (stamp_frame, tile_entries_stamp(layers)));
        }
    }
    for (coords, layer_id, opaque) in scratch.opaque_updates.drain(..) {
        buffer_pool.set_layer_opaque_color_by_id(coords, layer_id, opaque);
    }
}
