//! Style and layer eligibility checks for stationary dynamic-point GeoJSON updates.

use serde_json::Value as JsonValue;

use crate::style::{
    expression::reads_identity,
    feature_state::{layer_geometry_reads_feature_state, layer_paint_reads_feature_state},
    layer::{LayerPaint, StyleLayer, StyleProperty},
    source::Source,
    Style,
};

/// Returns true when `style_layer` uses [`CircleDynamicRadiusShader`](crate::render::shaders::CircleDynamicRadiusShader) (`2 * vertices` metadata).
pub(crate) fn uses_dynamic_circle_metadata(style: &Style, style_layer: &StyleLayer) -> bool {
    let Some(source_name) = style_layer.source.as_deref() else {
        return false;
    };
    layer_supports_dynamic_points(style_layer) && source_supports_dynamic_points(style, source_name)
}

/// Returns true when `source_name` is an unclustered, unfiltered GeoJSON source whose referencing
/// style layers all support in-place dynamic circle metadata updates.
pub(crate) fn source_supports_dynamic_points(style: &Style, source_name: &str) -> bool {
    let Some(Source::GeoJson(src)) = style.sources.get(source_name) else {
        return false;
    };
    if src.cluster || src.filter.is_some() || src.line_metrics {
        return false;
    }
    let mut has_any_layer = false;
    for layer in &style.layers {
        if layer.source.as_deref() != Some(source_name) {
            continue;
        }
        has_any_layer = true;
        if !layer_supports_dynamic_points(layer) {
            return false;
        }
    }
    has_any_layer
}

pub(super) fn source_layers_read_identity(style: &Style, source_name: &str) -> bool {
    style.layers.iter().any(|layer| {
        layer.source.as_deref() == Some(source_name)
            && match layer.paint.as_ref() {
                Some(LayerPaint::Circle(p)) => {
                    prop_reads_identity(p.circle_radius.as_ref())
                        || prop_reads_identity(p.circle_color.as_ref())
                        || prop_reads_identity(p.circle_opacity.as_ref())
                }
                _ => false,
            }
    })
}

fn layer_supports_dynamic_points(style_layer: &StyleLayer) -> bool {
    if style_layer.type_ != "circle"
        || style_layer.filter.is_some()
        || style_layer
            .unrecognized
            .layout
            .contains_key("circle-sort-key")
        || layer_paint_reads_feature_state(style_layer)
        || layer_geometry_reads_feature_state(style_layer)
    {
        return false;
    }
    let Some(LayerPaint::Circle(paint)) = style_layer.paint.as_ref() else {
        return false;
    };
    let radius_varies = !paint.radius().is_feature_constant();
    let color_varies = paint
        .circle_color
        .as_ref()
        .is_some_and(|c| !c.is_feature_constant());
    let opacity_varies = paint
        .circle_opacity
        .as_ref()
        .is_some_and(|o| !o.is_feature_constant());
    if !(radius_varies || color_varies || opacity_varies) {
        return false;
    }
    paint.stroke_color_per_feature().is_none()
        && paint
            .circle_stroke_width
            .as_ref()
            .is_none_or(StyleProperty::is_feature_constant)
        && paint
            .circle_blur
            .as_ref()
            .is_none_or(StyleProperty::is_feature_constant)
        && paint
            .circle_stroke_opacity
            .as_ref()
            .is_none_or(StyleProperty::is_feature_constant)
        && !prop_reads_spatial(paint.circle_radius.as_ref())
        && !prop_reads_spatial(paint.circle_color.as_ref())
        && !prop_reads_spatial(paint.circle_opacity.as_ref())
}

fn prop_reads_spatial<T>(prop: Option<&StyleProperty<T>>) -> bool {
    match prop {
        Some(StyleProperty::Expression(expr)) => json_reads_spatial(expr.source()),
        _ => false,
    }
}

fn json_reads_spatial(value: &JsonValue) -> bool {
    match value {
        JsonValue::Array(items) => {
            matches!(
                items.first().and_then(JsonValue::as_str),
                Some("within" | "distance")
            ) || items.iter().any(json_reads_spatial)
        }
        JsonValue::Object(map) => map.values().any(json_reads_spatial),
        _ => false,
    }
}

fn prop_reads_identity<T>(prop: Option<&StyleProperty<T>>) -> bool {
    match prop {
        Some(StyleProperty::Expression(expr)) => reads_identity(expr.source()),
        _ => false,
    }
}
