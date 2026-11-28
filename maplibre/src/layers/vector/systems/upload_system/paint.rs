//! Paint shared by every feature must use the view zoom, including overzoomed source tiles.
mod refresh;
mod slots;
pub(super) use refresh::PaintRefreshContext;
use rustc_hash::{FxHashMap, FxHashSet};
use slots::SlotMetaCache;

use super::{
    metadata::{
        feature_metadata_into, metadata_for_layer, paint_depends_on_bearing,
        paint_metadata_depends_on_zoom, pattern_runs, same_quantized_rgba,
        uniform_color_depends_on_zoom,
    },
    VectorPaintFrame,
};
use crate::{
    coords::WorldTileCoords,
    render::shaders::{FillShaderFeatureMetadata, ShaderLayerMetadata},
    style::layer::{LayerPaint, StyleLayer},
    tcs::tiles::Tiles,
    vector::{
        feature_paint::{evaluate_bucket_colors, TileFeaturePaintComponent},
        resource::PatternRuns,
        uniform_paint::{uniform_color, uses_layer_metadata_uniform_color},
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

const FLAG_UNSET: u8 = 0x80;

#[derive(Default)]
pub(super) struct PaintRefreshScratch {
    style_rev: Option<u64>,
    style_flags_union: u8,
    needs_feature_zoom_settle: bool,
    layer_flags: Vec<u8>,
    prev_meta: FxHashMap<(u32, u8), (VectorPaintFrame, ShaderLayerMetadata)>,
    meta_memo: FxHashMap<(u32, u8), (ShaderLayerMetadata, bool, bool)>,
    updated_layer_slots: FxHashSet<(u16, u64)>,
    color_memo: FxHashMap<u32, Option<[f32; 4]>>,
    feature_meta_scratch: Vec<FillShaderFeatureMetadata>,
    feature_zoom_updates: Vec<(WorldTileCoords, String, Option<PatternRuns>)>,
    feature_zoom_only_updates: Vec<(WorldTileCoords, String)>,
    opaque_updates: Vec<(WorldTileCoords, crate::style::StyleLayerId, bool)>,
    tile_paint_stamps: FxHashMap<WorldTileCoords, (VectorPaintFrame, u64)>,
    pending_uploads_key: Option<(u64, u64, u64, u64, usize)>,
    pending_uploads_sources: Option<std::sync::Arc<[WorldTileCoords]>>,
    slots: SlotMetaCache,
}

impl PaintRefreshScratch {
    /// Checks whether any tile in `source_tiles` still awaits vector GPU upload, caching clean frames.
    pub(super) fn check_pending_uploads(
        &mut self,
        tiles: &Tiles,
        stamp: (u64, u64),
        epochs: (u64, u64),
        source_tiles: &std::sync::Arc<[WorldTileCoords]>,
    ) -> bool {
        let key = (stamp.0, stamp.1, epochs.0, epochs.1, tiles.len());
        if self.pending_uploads_key == Some(key)
            && self
                .pending_uploads_sources
                .as_ref()
                .is_some_and(|prev| std::sync::Arc::ptr_eq(prev, source_tiles))
        {
            return false;
        }
        let pending = !source_tiles.iter().all(|&coords| {
            tiles
                .query::<(
                    &VectorLayerBucketComponent,
                    &crate::vector::content::LayerReplacements,
                )>(coords)
                .is_some_and(|(layers, rep)| {
                    layers.done && rep.vector.is_empty() && layers.uploaded_stamp == Some(stamp)
                })
        });
        if pending {
            self.pending_uploads_key = None;
            self.pending_uploads_sources = None;
        } else {
            self.pending_uploads_key = Some(key);
            self.pending_uploads_sources = Some(std::sync::Arc::clone(source_tiles));
        }
        pending
    }

    pub(super) fn prepare_refresh(
        &mut self,
        style: &crate::style::Style,
        active_tiles: &std::sync::Arc<[WorldTileCoords]>,
        eviction_gen: u64,
        (dirty_style_applied, refresh_paint, moving): (bool, bool, bool),
    ) -> (bool, bool) {
        if dirty_style_applied {
            self.slots.invalidate_all();
            self.invalidate_tiles();
        }
        self.slots.sync_generations(style.revision(), eviction_gen);
        let settle = !moving && self.needs_settle();
        let (run, tiles_changed) =
            self.slots
                .check_refresh_arc(active_tiles, refresh_paint, settle);
        if run {
            self.sync_style(style);
        }
        (run, tiles_changed)
    }

    #[inline]
    pub(super) fn record_uploaded_slot(&mut self, slot: (u16, u64), meta: ShaderLayerMetadata) {
        self.slots.record(slot, meta);
    }

    #[inline]
    pub(super) fn needs_settle(&self) -> bool {
        self.needs_feature_zoom_settle
    }

    fn invalidate_tiles(&mut self) {
        self.tile_paint_stamps.clear();
    }

    fn sync_style(&mut self, style: &crate::style::Style) {
        let style_rev = style.revision();
        if self.style_rev != Some(style_rev) {
            self.style_rev = Some(style_rev);
            self.layer_flags.clear();
            self.prev_meta.clear();
            self.tile_paint_stamps.clear();
            let max_idx = style.layers.iter().map(|l| l.index as usize).max();
            if let Some(max) = max_idx {
                self.layer_flags.resize(max + 1, FLAG_UNSET);
            }
            let mut union = 0_u8;
            for layer in &style.layers {
                let flags = layer_paint_flags(layer);
                if let Some(slot) = self.layer_flags.get_mut(layer.index as usize) {
                    *slot = flags;
                }
                union |= flags;
            }
            self.style_flags_union = union;
        }
    }

    #[inline]
    pub(super) fn flags_for_layer(&mut self, layer: &StyleLayer) -> u8 {
        let idx = layer.index as usize;
        if let Some(&flags) = self.layer_flags.get(idx)
            && flags != FLAG_UNSET
        {
            return flags;
        }
        let flags = layer_paint_flags(layer);
        if idx < 4096 {
            if idx >= self.layer_flags.len() {
                self.layer_flags.resize(idx + 1, FLAG_UNSET);
            }
            if let Some(slot) = self.layer_flags.get_mut(idx) {
                *slot = flags;
            }
        }
        flags
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

pub(super) fn feature_zoom_threshold(moving: bool, zoom_delta: f32) -> f32 {
    if !moving {
        1.0 / 32.0
    } else if zoom_delta >= 0.05 {
        0.25
    } else {
        0.125
    }
}

#[cfg(all(test, feature = "headless", feature = "thread-safe-futures"))]
pub(super) fn refresh_layer_paint(
    buffer_pool: &mut VectorBufferPool,
    queue: &crate::render::upload_queue::UploadQueue,
    ctx: PaintRefreshContext<'_>,
    scratch: &mut PaintRefreshScratch,
) {
    scratch.refresh_layer_paint(buffer_pool, queue, ctx);
}

#[cfg(test)]
mod tests;
