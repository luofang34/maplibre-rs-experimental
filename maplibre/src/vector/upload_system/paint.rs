//! Paint shared by every feature must use the view zoom, including overzoomed source tiles.
mod refresh;
mod slots;
mod uniform;
pub(super) use refresh::{refresh_layer_paint, PaintRefreshContext};
use rustc_hash::{FxHashMap, FxHashSet};
pub(super) use slots::SlotMetaCache;
pub(crate) use uniform::uniform_color;

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
    style::layer::{LayerPaint, StyleLayer},
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
    needs_feature_zoom_settle: bool,
    layer_flags: FxHashMap<u32, u8>,
    prev_meta: FxHashMap<(u32, u8), (VectorPaintFrame, ShaderLayerMetadata)>,
    meta_memo: FxHashMap<(u32, u8), (ShaderLayerMetadata, bool)>,
    updated_layer_slots: FxHashSet<(u16, u64)>,
    color_memo: FxHashMap<u32, Option<[f32; 4]>>,
    feature_meta_scratch: Vec<FillShaderFeatureMetadata>,
    feature_zoom_updates: Vec<(WorldTileCoords, String, Option<PatternRuns>)>,
    feature_zoom_only_updates: Vec<(WorldTileCoords, String)>,
    opaque_updates: Vec<(WorldTileCoords, crate::style::StyleLayerId, bool)>,
    pub(super) slots: SlotMetaCache,
}

impl PaintRefreshScratch {
    pub(super) fn needs_settle(&self) -> bool {
        self.needs_feature_zoom_settle
    }

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

pub(super) fn feature_zoom_threshold(moving: bool, zoom_delta: f32) -> f32 {
    if !moving {
        1.0 / 32.0
    } else if zoom_delta >= 0.05 {
        0.25
    } else {
        0.125
    }
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
