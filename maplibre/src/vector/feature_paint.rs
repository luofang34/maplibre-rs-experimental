//! Retained per-entry feature attributes for in-place GPU paint and `feature-state` updates.

/// Binary wire encoding of [`BucketFeatureEntry`] slices for worker transport.
pub mod wire;

use std::{collections::HashMap, sync::Arc};

use crate::{
    style::{
        expression::FeatureProperties,
        layer::{LayerPaint, StyleLayer},
        pattern_key,
    },
    tcs::tiles::TileComponent,
    vector::{pattern::per_feature_pattern, tessellation::circle_pack_colors},
};

/// Distinguishes how a bucket feature entry computes its RGBA metadata vector.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BucketEntryKind {
    /// Standard polygon fill, extrusion building part, circle quad, or heatmap point.
    Feature,
    /// Stroked path carrying its tessellated tile-space length for `line-gradient`.
    Line {
        /// Maximum contour path length in tile units recorded during stroke tessellation.
        length: f32,
    },
    /// One-pixel `fill-outline-color` stroke emitted immediately after a polygon's fill.
    FillOutline,
}

/// Source feature identity and attributes for one entry of `AvailableVectorLayerBucket::feature_indices`.
#[derive(Clone, Debug)]
pub struct BucketFeatureEntry {
    /// Raw MVT or numeric GeoJSON feature ID, if present.
    pub raw_id: Option<u64>,
    /// Shared feature properties (including internal `\u{1}state_id` and `\u{1}state:*` keys).
    pub properties: Arc<FeatureProperties>,
    /// Entry kind controlling whether `fill-outline-color` or `line-gradient` length applies.
    pub kind: BucketEntryKind,
}

/// Per-style-layer retained feature entries for a loaded tile, mirroring committed and pending
/// vector buckets so GPU `FillShaderFeatureMetadata` can be refreshed without re-tessellation.
#[derive(Default)]
pub(crate) struct TileFeaturePaintComponent {
    /// Entries matching the currently uploaded buckets in `VectorLayerBucketComponent`.
    pub committed: HashMap<String, Vec<BucketFeatureEntry>>,
    /// Entries matching staged replacement buckets in `LayerReplacements::vector`.
    pub pending: HashMap<String, Vec<BucketFeatureEntry>>,
}

impl TileComponent for TileFeaturePaintComponent {}

/// Re-evaluates per-entry RGBA feature metadata for `style_layer` at `style_zoom`, matching the
/// exact encoding produced by [`crate::vector::tessellation::ZeroTessellator`].
pub(crate) fn evaluate_bucket_colors(
    style_layer: &StyleLayer,
    entries: &[BucketFeatureEntry],
    style_zoom: f64,
) -> Vec<[f32; 4]> {
    let Some(paint) = style_layer.paint.as_ref() else {
        return vec![[0.0, 0.0, 0.0, 1.0]; entries.len()];
    };
    let opacity = paint.opacity();
    let mut fallback_color = [0.0, 0.0, 0.0, 1.0];
    let mut style_property = None;
    let mut outline_property = None;
    let mut pattern_property = None;
    let mut line_gradient = false;
    let mut circle_stroke_color = None;

    match paint {
        LayerPaint::Fill(fill) if fill.fill_pattern.is_some() => {
            fallback_color = [1.0; 4];
            pattern_property = per_feature_pattern(paint);
        }
        LayerPaint::Fill(fill) => {
            style_property = fill.fill_color.as_ref();
            outline_property = fill.outline_color();
        }
        LayerPaint::FillExtrusion(ext) if ext.fill_extrusion_pattern.is_some() => {
            fallback_color = [1.0; 4];
            pattern_property = per_feature_pattern(paint);
        }
        LayerPaint::FillExtrusion(ext) => {
            style_property = ext.fill_extrusion_color.as_ref();
        }
        LayerPaint::Circle(circle) => {
            style_property = circle.circle_color.as_ref();
            circle_stroke_color = circle.stroke_color_per_feature();
        }
        LayerPaint::Line(line) => {
            pattern_property = per_feature_pattern(paint);
            style_property = line.line_color.as_ref();
            line_gradient = line.line_gradient.is_some();
        }
        LayerPaint::Background(bg) => {
            style_property = bg.background_color.as_ref();
        }
        _ => {}
    }

    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let props = entry.properties.as_ref();
        let feat_opacity = opacity.as_ref().map_or(1.0, |prop| {
            prop.evaluate_for(props, style_zoom)
                .unwrap_or(1.0)
                .clamp(0.0, 1.0)
        });
        match entry.kind {
            BucketEntryKind::FillOutline => {
                let color = outline_property
                    .as_ref()
                    .and_then(|prop| prop.evaluate_for(props, style_zoom))
                    .map_or([0.0, 0.0, 0.0, 0.0], |c| {
                        [c.r, c.g, c.b, c.a * feat_opacity]
                    });
                out.push(color);
            }
            BucketEntryKind::Feature | BucketEntryKind::Line { .. } => {
                let mut color = style_property
                    .and_then(|prop| prop.evaluate_for(props, style_zoom))
                    .map_or(fallback_color, |c| [c.r, c.g, c.b, c.a]);
                if line_gradient {
                    let length = match entry.kind {
                        BucketEntryKind::Line { length } => length,
                        _ => 0.0,
                    };
                    color = [length, 0.0, 0.0, 1.0];
                }
                color[3] *= feat_opacity;
                if let Some(prop) = &pattern_property {
                    color[0] = pattern_key::pattern_value(
                        prop.evaluate_for(props, style_zoom)
                            .as_ref()
                            .map(|name| name.0.as_str()),
                    );
                }
                if let Some(stroke_prop) = circle_stroke_color {
                    let stroke = stroke_prop
                        .evaluate_for(props, style_zoom)
                        .map_or([0.0, 0.0, 0.0, 1.0], |c| c.to_array());
                    color = circle_pack_colors(color, stroke);
                }
                out.push(color);
            }
        }
    }
    out
}
