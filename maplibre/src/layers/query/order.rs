//! Draw-order sorting and candidate layer preparation for `queryRenderedFeatures`.

use std::{
    cmp::Reverse,
    collections::{BTreeMap, HashSet},
};

use super::{ground, QueriedFeature};
use crate::style::{
    circle::CirclePaint,
    expression::FeatureProperties,
    filter::Filter,
    heatmap::HeatmapPaint,
    layer::{layer_translate_pixels, LayerPaint, LinePaint, StyleLayer},
};

const DEFAULT_RADIUS: f64 = 5.0;
const DEFAULT_HEATMAP_RADIUS: f64 = 30.0;

/// The circle's radius and stroke width, in pixels, for one feature.
pub(super) fn size_pixels(paint: &CirclePaint, properties: &FeatureProperties, zoom: f64) -> f64 {
    let value = |property: &Option<crate::style::property::StyleProperty<f32>>, default: f64| {
        property
            .as_ref()
            .and_then(|value| value.evaluate_for(properties, zoom))
            .map_or(default, f64::from)
    };
    value(&paint.circle_radius, DEFAULT_RADIUS) + value(&paint.circle_stroke_width, 0.0)
}

pub(super) fn heatmap_radius(
    paint: &HeatmapPaint,
    properties: &FeatureProperties,
    zoom: f64,
) -> f64 {
    paint
        .heatmap_radius
        .as_ref()
        .and_then(|radius| radius.evaluate_for(properties, zoom))
        .map_or(DEFAULT_HEATMAP_RADIUS, f64::from)
}

/// How far from the query, in screen pixels, a heatmap point can be and still meet it.
fn heatmap_margin_pixels(paint: &HeatmapPaint, zoom: f64) -> f64 {
    const BY_FEATURE: f64 = 64.0;
    let varies = paint
        .heatmap_radius
        .as_ref()
        .is_some_and(|radius| !radius.is_feature_constant());
    heatmap_radius(paint, &FeatureProperties::default(), zoom)
        + if varies { BY_FEATURE } else { 0.0 }
}

/// How far from the query, in screen pixels, a circle of the layer can be centred and still
/// meet it: its size, doubled for the circles a pitched view brings nearer, and more where the
/// size varies by feature, which is not known before the feature is.
fn circle_margin_pixels(paint: &CirclePaint, zoom: f64) -> f64 {
    const BY_FEATURE: f64 = 64.0;
    let varies = [&paint.circle_radius, &paint.circle_stroke_width]
        .into_iter()
        .flatten()
        .any(|property| !property.is_feature_constant());
    2.0 * size_pixels(paint, &FeatureProperties::default(), zoom)
        + if varies { BY_FEATURE } else { 0.0 }
}

/// Features topmost first, as GL JS `Style.queryRenderedFeatures` orders them: layer by layer
/// from the top, where each fill extrusion layer gives out the nearest remaining extrusions
/// for as long as the nearest is in that layer or above it.
pub(super) fn in_drawing_order(
    found: Vec<(u32, Option<f64>, QueriedFeature)>,
) -> Vec<QueriedFeature> {
    let (mut extruded, flat): (Vec<_>, Vec<_>) =
        found.into_iter().partition(|(_, depth, _)| depth.is_some());
    // Nearest last, so the nearest is popped first.
    extruded.sort_by(|a, b| b.1.unwrap_or(0.0).total_cmp(&a.1.unwrap_or(0.0)));
    let extrusion_layers: HashSet<u32> = extruded.iter().map(|(layer, _, _)| *layer).collect();
    let mut layers: Vec<u32> = flat
        .iter()
        .map(|(layer, _, _)| *layer)
        .chain(extrusion_layers.iter().copied())
        .collect();
    layers.sort_unstable_by_key(|layer| Reverse(*layer));
    layers.dedup();
    let mut flat_by_layer: BTreeMap<u32, Vec<QueriedFeature>> = BTreeMap::new();
    for (layer, _, feature) in flat {
        flat_by_layer.entry(layer).or_default().push(feature);
    }
    let mut ordered = Vec::new();
    for layer in layers {
        if extrusion_layers.contains(&layer) {
            while extruded
                .last()
                .is_some_and(|(nearest, _, _)| *nearest >= layer)
            {
                if let Some((_, _, feature)) = extruded.pop() {
                    ordered.push(feature);
                }
            }
        }
        if let Some(features) = flat_by_layer.remove(&layer) {
            ordered.extend(features);
        }
    }
    ordered
}

/// A layer a fill, line, circle, heatmap or extrusion feature can be reported for, with its
/// filter parsed once.
pub(super) struct Candidate<'a> {
    pub(super) layer: &'a StyleLayer,
    pub(super) filter: Option<Filter>,
    /// Maximum query search margin around the cursor, in screen pixels.
    pub(super) margin_pixels: f64,
    /// Where the layer draws its features from where they are, in screen pixels along the
    /// map's axes: its `*-translate`.
    pub(super) translate: [f64; 2],
}

impl<'a> Candidate<'a> {
    /// A layer whose filter cannot be parsed draws nothing, so it also matches nothing.
    pub(super) fn new(layer: &'a StyleLayer, zoom: f64, bearing: f64) -> Option<Self> {
        let filter = match &layer.filter {
            Some(filter) => Some(Filter::parse(filter).ok()?),
            None => None,
        };
        let reach_pixels = match &layer.paint {
            Some(LayerPaint::Line(paint)) => line_margin_pixels(paint, zoom),
            Some(LayerPaint::Circle(paint)) => circle_margin_pixels(paint, zoom),
            Some(LayerPaint::Heatmap(paint)) => heatmap_margin_pixels(paint, zoom),
            _ => 0.0,
        };
        let translate = layer_translate_pixels(layer.paint.as_ref(), zoom, bearing);
        Some(Self {
            layer,
            filter,
            margin_pixels: reach_pixels + translate[0].hypot(translate[1]),
            translate,
        })
    }
}

/// The farthest a line layer reaches from its features: half its width and its offset, at their
/// widest where they vary by feature.
fn line_margin_pixels(paint: &LinePaint, zoom: f64) -> f64 {
    // A width or offset set by feature is not known before the feature is; this much covers
    // what styles give them.
    const BY_FEATURE: f64 = 64.0;
    let [half_width, offset] = ground::line_reach(paint, &Default::default(), zoom);
    let varies = [&paint.line_width, &paint.line_gap_width, &paint.line_offset]
        .into_iter()
        .flatten()
        .any(|property| !property.is_feature_constant());
    half_width + offset.abs() + if varies { BY_FEATURE } else { 0.0 }
}
