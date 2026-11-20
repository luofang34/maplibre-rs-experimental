//! Paint shared by every feature must use the view zoom, including overzoomed source tiles.
use rustc_hash::{FxHashMap, FxHashSet};

use super::{
    metadata::{
        feature_metadata_into, metadata_for_layer, paint_depends_on_bearing,
        paint_metadata_depends_on_zoom, pattern_runs, same_quantized_rgba,
        uniform_color_depends_on_zoom, uses_layer_metadata_uniform_color,
    },
    VectorPaintFrame,
};
use crate::{
    coords::WorldTileCoords,
    render::shaders::{FillShaderFeatureMetadata, ShaderLayerMetadata},
    style::layer::{LayerPaint, StyleLayer, StyleProperty},
    tcs::tiles::Tiles,
    vector::{
        feature_paint::{evaluate_bucket_colors, TileFeaturePaintComponent},
        resource::PatternRuns,
        VectorBufferPool, VectorLayerBucket, VectorLayerBucketComponent,
    },
};

/// Whether `layer`'s feature colors/patterns/outlines (excluding uniform layer opacity) depend
/// on view zoom.
#[inline]
fn feature_colors_depend_on_zoom(layer: &StyleLayer) -> bool {
    let Some(paint) = layer.paint.as_ref() else {
        return false;
    };
    if crate::vector::pattern::per_feature_pattern(paint).is_some_and(|p| !p.is_zoom_constant()) {
        return true;
    }
    match paint {
        LayerPaint::Fill(fill) => {
            fill.fill_color
                .as_ref()
                .is_some_and(|p| !p.is_zoom_constant())
                || fill
                    .outline_color()
                    .as_ref()
                    .is_some_and(|p| !p.is_zoom_constant())
        }
        LayerPaint::FillExtrusion(ext) => ext
            .fill_extrusion_color
            .as_ref()
            .is_some_and(|p| !p.is_zoom_constant()),
        LayerPaint::Circle(circle) => {
            circle
                .circle_color
                .as_ref()
                .is_some_and(|p| !p.is_zoom_constant())
                || circle
                    .stroke_color_per_feature()
                    .is_some_and(|p| !p.is_zoom_constant())
        }
        LayerPaint::Line(line) => line
            .line_color
            .as_ref()
            .is_some_and(|p| !p.is_zoom_constant()),
        LayerPaint::Background(bg) => bg
            .background_color
            .as_ref()
            .is_some_and(|p| !p.is_zoom_constant()),
        _ => false,
    }
}

#[inline]
fn quantized_layer_opacity(layer: &StyleLayer, zoom: f32) -> Option<u8> {
    let opacity = layer
        .paint
        .as_ref()?
        .opacity()?
        .evaluate_at_zoom(f64::from(zoom))?
        .clamp(0.0, 1.0);
    Some((opacity * 255.0).round() as u8)
}

/// Whether `layer`'s per-feature GPU metadata depends on zoom (meaning zooming changes
/// already-uploaded vertex colors or per-feature patterns/outlines).
#[inline]
pub(super) fn feature_paint_depends_on_zoom(layer: &StyleLayer) -> bool {
    let Some(paint) = layer.paint.as_ref() else {
        return false;
    };
    paint
        .opacity()
        .as_ref()
        .is_some_and(|p| !p.is_zoom_constant())
        || feature_colors_depend_on_zoom(layer)
}

#[derive(Default)]
pub(super) struct PaintRefreshScratch {
    style_rev: Option<u64>,
    style_flags_union: u8,
    layer_flags: FxHashMap<u32, u8>,
    prev_meta: FxHashMap<(u32, u8), (VectorPaintFrame, ShaderLayerMetadata)>,
    meta_memo: FxHashMap<(u32, u8), (ShaderLayerMetadata, bool)>,
    updated_layer_slots: FxHashSet<(u16, u64)>,
    color_memo: FxHashMap<u32, Option<[f32; 4]>>,
    feature_meta_scratch: Vec<FillShaderFeatureMetadata>,
    feature_zoom_updates: Vec<(WorldTileCoords, String, Option<PatternRuns>)>,
    feature_zoom_only_updates: Vec<(WorldTileCoords, String)>,
    opaque_updates: Vec<(WorldTileCoords, crate::style::StyleLayerId, bool)>,
}

impl PaintRefreshScratch {
    pub(super) fn sync_style(&mut self, style: &crate::style::Style) {
        let style_rev = style.revision();
        if self.style_rev != Some(style_rev) {
            self.style_rev = Some(style_rev);
            self.layer_flags.clear();
            self.prev_meta.clear();
            let mut union = 0_u8;
            for layer in &style.layers {
                let flags = layer_paint_flags(layer);
                self.layer_flags.insert(layer.index, flags);
                union |= flags;
            }
            self.style_flags_union = union;
        }
    }

    fn clear(&mut self) {
        self.meta_memo.clear();
        self.updated_layer_slots.clear();
        self.color_memo.clear();
        self.feature_meta_scratch.clear();
        self.feature_zoom_updates.clear();
        self.feature_zoom_only_updates.clear();
        self.opaque_updates.clear();
    }
}

#[inline]
fn layer_paint_flags(layer: &StyleLayer) -> u8 {
    let paint = layer.paint.as_ref();
    u8::from(paint_metadata_depends_on_zoom(paint))
        | (u8::from(paint_depends_on_bearing(paint)) << 1)
        | (u8::from(matches!(paint, Some(LayerPaint::FillExtrusion(_)))) << 2)
        | (u8::from(feature_paint_depends_on_zoom(layer)) << 3)
        | (u8::from(feature_colors_depend_on_zoom(layer)) << 4)
        | (u8::from(uses_layer_metadata_uniform_color(layer)) << 5)
        | (u8::from(uniform_color_depends_on_zoom(layer)) << 6)
}

pub(super) fn refresh_layer_paint(
    buffer_pool: &mut VectorBufferPool,
    queue: &crate::render::upload_queue::UploadQueue,
    tiles: &Tiles,
    source_tiles: &[WorldTileCoords],
    frame: VectorPaintFrame,
    previous: Option<VectorPaintFrame>,
    scratch: &mut PaintRefreshScratch,
) {
    let VectorPaintFrame { zoom, .. } = frame;
    let zoom_changed = previous.is_none_or(|prev| prev.zoom != zoom);
    let bearing_changed = previous.is_none_or(|prev| prev.bearing != frame.bearing);
    let light_changed = previous.is_none_or(|prev| prev.light != frame.light);
    if !zoom_changed && !bearing_changed && !light_changed {
        return;
    }
    let active_flags = (u8::from(zoom_changed) * (1 | 8 | 64))
        | (u8::from(bearing_changed) * 2)
        | (u8::from(light_changed) * 4);
    if previous.is_some() && (scratch.style_flags_union & active_flags) == 0 {
        return;
    }
    let continuous_zoom =
        previous.is_some_and(|prev| prev.zoom != zoom && (zoom - prev.zoom).abs() < 0.05);
    let feature_zoom_threshold = if continuous_zoom { 0.125 } else { 1.0 / 32.0 };
    scratch.clear();
    let PaintRefreshScratch {
        layer_flags,
        prev_meta,
        meta_memo,
        updated_layer_slots,
        color_memo,
        feature_meta_scratch,
        feature_zoom_updates,
        feature_zoom_only_updates,
        opaque_updates,
        ..
    } = scratch;

    for coords in source_tiles {
        let tile_z = u8::from(coords.z);
        for entry in buffer_pool
            .index()
            .get_layers(*coords)
            .into_iter()
            .flatten()
        {
            let flags = *layer_flags
                .entry(entry.style_layer.index)
                .or_insert_with(|| layer_paint_flags(&entry.style_layer));
            let meta_dirty = (zoom_changed && (flags & 1) != 0)
                || (bearing_changed && (flags & 2) != 0)
                || (light_changed && (flags & 4) != 0);
            if meta_dirty {
                let key = (entry.style_layer.index, tile_z);
                let (metadata, changed) = *meta_memo.entry(key).or_insert_with(|| {
                    let metadata = metadata_for_layer(&entry.style_layer, *coords, frame);
                    let changed = match (previous, prev_meta.get(&key)) {
                        (Some(prev_frame), Some((cached_frame, cached_meta)))
                            if *cached_frame == prev_frame =>
                        {
                            bytemuck::bytes_of(cached_meta) != bytemuck::bytes_of(&metadata)
                        }
                        (Some(prev_frame), _) => {
                            bytemuck::bytes_of(&metadata_for_layer(
                                &entry.style_layer,
                                *coords,
                                prev_frame,
                            )) != bytemuck::bytes_of(&metadata)
                        }
                        (None, _) => true,
                    };
                    prev_meta.insert(key, (frame, metadata));
                    (metadata, changed)
                });
                if changed {
                    if entry.style_layer.type_ == "fill"
                        && (flags & 32) != 0
                        && !matches!(
                            entry.style_layer.paint.as_ref(),
                            Some(LayerPaint::Fill(f)) if f.fill_pattern.is_some()
                        )
                    {
                        opaque_updates.push((
                            *coords,
                            entry.style_layer_id,
                            metadata.uniform_color[3] >= 1.0,
                        ));
                    }
                    if updated_layer_slots
                        .insert((entry.page(), entry.layer_metadata_buffer_range().start))
                    {
                        buffer_pool.update_layer_metadata(queue, entry, metadata);
                    }
                }
            }
            if zoom_changed
                && !entry.is_uniform_feature_metadata()
                && !entry.is_dynamic_circle_metadata()
                && (flags & 8) != 0
                && entry
                    .evaluated_zoom()
                    .is_none_or(|evaluated| (zoom - evaluated).abs() >= feature_zoom_threshold)
            {
                if let Some(evaluated) = entry.evaluated_zoom() {
                    if (flags & 16) == 0
                        && quantized_layer_opacity(&entry.style_layer, zoom)
                            .zip(quantized_layer_opacity(&entry.style_layer, evaluated))
                            .is_some_and(|(a, b)| a == b)
                    {
                        feature_zoom_only_updates.push((*coords, entry.style_layer.id.clone()));
                        continue;
                    }
                }
                if let Some((paint_store, buckets)) = tiles
                    .query::<(&TileFeaturePaintComponent, &VectorLayerBucketComponent)>(*coords)
                {
                    let entries = paint_store.committed.get(&entry.style_layer.id);
                    let bucket = buckets.layers.iter().find_map(|layer| match layer {
                        VectorLayerBucket::AvailableLayer(avail)
                            if avail.style_layer_id == entry.style_layer.id =>
                        {
                            Some(avail)
                        }
                        _ => None,
                    });
                    if let (Some(entries), Some(bucket)) = (entries, bucket) {
                        if !bucket.feature_indices.is_empty() {
                            let allocated_count = (entry.feature_metadata_buffer_range().end
                                - entry.feature_metadata_buffer_range().start)
                                as usize
                                / size_of::<FillShaderFeatureMetadata>();
                            let colors = evaluate_bucket_colors(
                                &entry.style_layer,
                                entries,
                                f64::from(zoom),
                            );
                            let fallback_color: [f32; 4] = entry
                                .style_layer
                                .paint
                                .as_ref()
                                .and_then(LayerPaint::get_color)
                                .map(Into::into)
                                .unwrap_or([0.0, 0.0, 0.0, 1.0]);
                            feature_metadata_into(
                                feature_meta_scratch,
                                &entry.style_layer,
                                &bucket.feature_indices,
                                &colors,
                                fallback_color,
                            );
                            if feature_meta_scratch.len() == allocated_count {
                                buffer_pool.update_feature_metadata(
                                    queue,
                                    entry,
                                    feature_meta_scratch,
                                );
                                let runs = entry
                                    .style_layer
                                    .paint
                                    .as_ref()
                                    .and_then(crate::vector::pattern::per_feature_pattern)
                                    .filter(|_| !bucket.buffer.buffer.indices.is_empty())
                                    .map(|_| {
                                        pattern_runs(
                                            &bucket.feature_indices,
                                            &colors,
                                            &bucket.buffer.buffer.indices,
                                        )
                                    });
                                feature_zoom_updates.push((
                                    *coords,
                                    entry.style_layer.id.clone(),
                                    runs,
                                ));
                                if entry.style_layer.type_ == "fill"
                                    && !matches!(
                                        entry.style_layer.paint.as_ref(),
                                        Some(LayerPaint::Fill(f)) if f.fill_pattern.is_some()
                                    )
                                {
                                    opaque_updates.push((
                                        *coords,
                                        entry.style_layer_id,
                                        !feature_meta_scratch.is_empty()
                                            && feature_meta_scratch
                                                .iter()
                                                .all(|fm| fm.color[3] == 255),
                                    ));
                                }
                            }
                        }
                    }
                }
            }
            // Uniform fill and line colours already ride in `ShaderLayerMetadata::stroke_color`.
            if !zoom_changed || (flags & 32) != 0 || (flags & 64) == 0 {
                continue;
            }
            let changed_color = *color_memo
                .entry(entry.style_layer.index)
                .or_insert_with(|| {
                    let color = uniform_color(&entry.style_layer, f64::from(zoom))?;
                    let unchanged = previous
                        .and_then(|prev| uniform_color(&entry.style_layer, f64::from(prev.zoom)))
                        .is_some_and(|prev_color| same_quantized_rgba(prev_color, color));
                    (!unchanged).then_some(color)
                });
            if let Some(color) = changed_color {
                buffer_pool.fill_feature_metadata(
                    queue,
                    entry,
                    FillShaderFeatureMetadata::from_rgba(color),
                );
                if entry.style_layer.type_ == "fill" {
                    opaque_updates.push((*coords, entry.style_layer_id, color[3] >= 1.0));
                }
            }
        }
    }

    for (coords, layer_id, runs) in feature_zoom_updates.drain(..) {
        buffer_pool.set_layer_evaluated_zoom(coords, &layer_id, zoom);
        if runs.is_some() {
            buffer_pool.set_pattern_runs(coords, &layer_id, runs);
        }
        buffer_pool.bump_layer_paint_revision(coords, &layer_id);
    }
    for (coords, layer_id) in feature_zoom_only_updates.drain(..) {
        buffer_pool.set_layer_evaluated_zoom(coords, &layer_id, zoom);
    }
    for (coords, layer_id, opaque) in opaque_updates.drain(..) {
        buffer_pool.set_layer_opaque_color_by_id(coords, layer_id, opaque);
    }
}

/// White with the opacity of a fill that repeats an image, unless the opacity varies by feature.
fn pattern_color(opacity: Option<&StyleProperty<f32>>, zoom: f64) -> Option<[f32; 4]> {
    if opacity.is_some_and(|opacity| !opacity.is_feature_constant()) {
        return None;
    }
    let opacity = opacity
        .and_then(|opacity| opacity.evaluate_at_zoom(zoom))
        .unwrap_or(1.0)
        .clamp(0.0, 1.0);
    Some([1.0, 1.0, 1.0, opacity])
}

pub(crate) fn uniform_color(layer: &StyleLayer, zoom: f64) -> Option<[f32; 4]> {
    let Some(layer_paint) = layer.paint.as_ref() else {
        return uses_layer_metadata_uniform_color(layer).then_some([0.0, 0.0, 0.0, 1.0]);
    };
    let (color, opacity) = match layer_paint {
        // A pattern takes only the opacity of the fill.
        LayerPaint::Fill(paint) if paint.fill_pattern.is_some() => {
            // A pattern that varies by feature rides in the feature colours.
            if super::super::pattern::per_feature_pattern(layer_paint).is_some() {
                return None;
            }
            return pattern_color(paint.fill_opacity.as_ref(), zoom);
        }
        // The outline is a feature of its own, with its own colour.
        LayerPaint::Fill(paint) if paint.fill_outline_color.is_some() => return None,
        LayerPaint::Fill(paint) => (&paint.fill_color, &paint.fill_opacity),
        // A gradient's colour comes from its ramp, so the feature colours carry the length.
        LayerPaint::Line(paint) if paint.line_gradient.is_some() => return None,
        // A pattern that varies by feature rides in the feature colours.
        LayerPaint::Line(paint) if paint.line_pattern.is_some() => {
            if super::super::pattern::per_feature_pattern(layer_paint).is_some() {
                return None;
            }
            (&paint.line_color, &paint.line_opacity)
        }
        LayerPaint::Line(paint) => (&paint.line_color, &paint.line_opacity),
        LayerPaint::FillExtrusion(paint) if paint.fill_extrusion_pattern.is_some() => {
            if super::super::pattern::per_feature_pattern(layer_paint).is_some() {
                return None;
            }
            return if paint.fill_extrusion_translucent == Some(true) {
                pattern_color(paint.fill_extrusion_opacity.as_ref(), zoom)
            } else {
                Some([1.0, 1.0, 1.0, 1.0])
            };
        }
        LayerPaint::FillExtrusion(paint) => {
            let opacity = if paint.fill_extrusion_translucent == Some(true) {
                &paint.fill_extrusion_opacity
            } else {
                &None
            };
            (&paint.fill_extrusion_color, opacity)
        }
        LayerPaint::Circle(paint) => {
            if paint.stroke_color_per_feature().is_some() {
                return None;
            }
            (&paint.circle_color, &paint.circle_opacity)
        }
        LayerPaint::Heatmap(_) => return Some([0.0, 0.0, 0.0, 1.0]),
        _ => return None,
    };
    if color.as_ref().is_some_and(|v| !v.is_feature_constant())
        || opacity.as_ref().is_some_and(|v| !v.is_feature_constant())
    {
        return None;
    }
    let color = color
        .as_ref()
        .and_then(|v| v.evaluate_at_zoom(zoom))
        .unwrap_or_default();
    let opacity = opacity
        .as_ref()
        .and_then(|v| v.evaluate_at_zoom(zoom))
        .unwrap_or(1.0)
        .clamp(0.0, 1.0);
    Some([color.r, color.g, color.b, color.a * opacity])
}

#[cfg(test)]
mod tests;

#[derive(Default)]
struct PaintZoom(Option<f64>);

pub(super) fn stabilize_zoom(world: &mut crate::tcs::world::World, requested: f64) -> f64 {
    let state = world.resources.get_or_init_mut::<PaintZoom>();
    let zoom = next_zoom(state.0, requested);
    state.0 = Some(zoom);
    zoom
}

fn next_zoom(previous: Option<f64>, requested: f64) -> f64 {
    previous
        .filter(|previous| (requested - previous).abs() < 0.125)
        .unwrap_or_else(|| super::drape_paint_zoom(requested))
}

pub(crate) fn current_zoom(world: &crate::tcs::world::World, fallback: f64) -> f64 {
    world
        .resources
        .get::<PaintZoom>()
        .and_then(|state| state.0)
        .unwrap_or_else(|| super::drape_paint_zoom(fallback))
}
