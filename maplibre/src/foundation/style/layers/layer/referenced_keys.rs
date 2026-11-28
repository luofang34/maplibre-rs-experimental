//! Collects MVT feature property keys referenced by style layers so decoding and runtime
//! retention can skip unused attributes.

use rustc_hash::FxHashSet;

use crate::style::{
    expression::{convert_filter, convert_token_string, Expression, FeatureProperty, Value},
    layer::{LayerPaint, RasterResampling, StyleLayer, StyleProperty},
    translation::Translation,
};

#[cfg(test)]
mod tests;

const SYMBOL_RUNTIME_PROPERTIES: &[&str] = &[
    "text-color",
    "text-halo-color",
    "text-halo-width",
    "text-halo-blur",
    "text-opacity",
    "icon-color",
    "icon-halo-color",
    "icon-halo-width",
    "icon-halo-blur",
    "icon-opacity",
    "icon-size",
    "text-allow-overlap",
    "icon-allow-overlap",
    "text-ignore-placement",
    "icon-ignore-placement",
    "text-optional",
    "icon-optional",
    "text-padding",
    "icon-padding",
    "text-variable-anchor",
    "text-variable-anchor-offset",
    "text-radial-offset",
    "text-offset",
    "icon-offset",
    "icon-text-fit",
    "symbol-height-offset",
    "text-height-offset",
    "icon-height-offset",
    "symbol-z-elevate",
    "symbol-z-offset",
    "symbol-sort-key",
];

/// Returns the set of MVT feature property keys needed by `layers` (plus `promote_id_key`),
/// or `None` when any layer performs a dynamic property lookup or reads all `["properties"]`.
pub(crate) fn collect_layer_referenced_keys(
    layers: &[&StyleLayer],
    promote_id_key: Option<&str>,
) -> Option<FxHashSet<String>> {
    let mut keys = FxHashSet::default();
    if let Some(promote) = promote_id_key {
        keys.insert(promote.to_owned());
    }
    for layer in layers {
        if !collect_style_layer_keys(layer, &mut keys) {
            return None;
        }
    }
    Some(keys)
}

/// Returns the set of MVT feature property keys evaluated on `layer` AFTER symbol tessellation
/// (placement, collision, and runtime style paint), or `None` on dynamic lookup / `["properties"]`.
pub(crate) fn symbol_runtime_referenced_keys(
    layer: &StyleLayer,
    promote_id_key: Option<&str>,
) -> Option<FxHashSet<String>> {
    let mut keys = FxHashSet::default();
    if let Some(promote) = promote_id_key {
        keys.insert(promote.to_owned());
    }
    let Some(LayerPaint::Symbol(paint)) = &layer.paint else {
        return Some(keys);
    };
    if !collect_prop(&paint.text_size, &mut keys) {
        return None;
    }
    for &prop_name in SYMBOL_RUNTIME_PROPERTIES {
        if let Some(value) = paint.properties.get(prop_name) {
            if !walk_json(value, false, &mut keys) {
                return None;
            }
        }
        if let Some(value) = layer
            .unrecognized
            .paint
            .get(prop_name)
            .or_else(|| layer.unrecognized.layout.get(prop_name))
        {
            if !walk_json(value, false, &mut keys) {
                return None;
            }
        }
    }
    Some(keys)
}

fn collect_style_layer_keys(layer: &StyleLayer, keys: &mut FxHashSet<String>) -> bool {
    if layer.type_ == "fill-extrusion" {
        keys.insert("hide_3d".to_owned());
        keys.insert("render_min_height".to_owned());
        keys.insert("render_height".to_owned());
    }
    if let Some(filter) = &layer.filter {
        let converted = convert_filter(filter);
        if !walk_json(&converted, false, keys) {
            return false;
        }
    }
    for (k, v) in &layer.unrecognized.layout {
        if !walk_json(v, is_token_property(k), keys) {
            return false;
        }
    }
    for (k, v) in &layer.unrecognized.paint {
        if !walk_json(v, is_token_property(k), keys) {
            return false;
        }
    }
    let Some(paint) = &layer.paint else {
        return true;
    };
    collect_layer_paint_keys(paint, keys)
}

fn collect_layer_paint_keys(paint: &LayerPaint, keys: &mut FxHashSet<String>) -> bool {
    match paint {
        LayerPaint::Background(p) => {
            collect_prop(&p.background_color, keys)
                && collect_opt_json(&p.background_pattern, true, keys)
                && collect_prop(&p.background_opacity, keys)
        }
        LayerPaint::Fill(p) => {
            collect_prop(&p.fill_color, keys)
                && collect_prop(&p.fill_outline_color, keys)
                && collect_prop(&p.fill_opacity, keys)
                && collect_opt_json(&p.fill_pattern, true, keys)
                && collect_opt_json(&p.fill_antialias, false, keys)
                && collect_translation(&p.fill_translate, keys)
        }
        LayerPaint::FillExtrusion(_) | LayerPaint::Line(_) | LayerPaint::Circle(_) => {
            collect_vector_paint_keys(paint, keys)
        }
        LayerPaint::Heatmap(p) => {
            collect_prop(&p.heatmap_radius, keys)
                && collect_prop(&p.heatmap_weight, keys)
                && collect_prop(&p.heatmap_intensity, keys)
                && collect_prop(&p.heatmap_opacity, keys)
                && collect_prop(&p.heatmap_color, keys)
        }
        LayerPaint::Symbol(_)
        | LayerPaint::Raster(_)
        | LayerPaint::Hillshade(_)
        | LayerPaint::ColorRelief(_) => collect_symbol_or_raster_paint_keys(paint, keys),
        LayerPaint::Custom(_) => true,
    }
}

fn collect_vector_paint_keys(paint: &LayerPaint, keys: &mut FxHashSet<String>) -> bool {
    match paint {
        LayerPaint::FillExtrusion(p) => {
            collect_prop(&p.fill_extrusion_color, keys)
                && collect_opt_json(&p.fill_extrusion_pattern, true, keys)
                && collect_prop(&p.fill_extrusion_opacity, keys)
                && collect_prop(&p.fill_extrusion_height, keys)
                && collect_prop(&p.fill_extrusion_base, keys)
                && collect_prop(&p.fill_extrusion_base_datum, keys)
                && collect_prop(&p.fill_extrusion_height_datum, keys)
                && collect_prop(&p.fill_extrusion_part, keys)
                && collect_prop(&p.fill_extrusion_rounded_corner_distance, keys)
                && collect_translation(&p.fill_extrusion_translate, keys)
        }
        LayerPaint::Line(p) => {
            collect_prop(&p.line_color, keys)
                && collect_prop(&p.line_width, keys)
                && collect_prop(&p.line_opacity, keys)
                && collect_translation(&p.line_translate, keys)
                && collect_prop(&p.line_gradient, keys)
                && collect_prop(&p.line_offset, keys)
                && collect_prop(&p.line_gap_width, keys)
                && collect_prop(&p.line_blur, keys)
                && collect_opt_json(&p.line_pattern, true, keys)
                && collect_opt_json(&p.line_dasharray, false, keys)
                && collect_prop(&p.line_elevation, keys)
                && collect_prop(&p.line_elevation_datum, keys)
        }
        LayerPaint::Circle(p) => {
            collect_prop(&p.circle_radius, keys)
                && collect_prop(&p.circle_color, keys)
                && collect_prop(&p.circle_blur, keys)
                && collect_prop(&p.circle_opacity, keys)
                && collect_prop(&p.circle_stroke_width, keys)
                && collect_prop(&p.circle_stroke_color, keys)
                && collect_prop(&p.circle_stroke_opacity, keys)
                && collect_translation(&p.circle_translate, keys)
        }
        _ => true,
    }
}

fn collect_symbol_or_raster_paint_keys(paint: &LayerPaint, keys: &mut FxHashSet<String>) -> bool {
    match paint {
        LayerPaint::Symbol(p) => {
            if !collect_prop(&p.text_field, keys) || !collect_prop(&p.text_size, keys) {
                return false;
            }
            for (k, v) in &p.properties {
                if !walk_json(v, is_token_property(k), keys) {
                    return false;
                }
            }
            true
        }
        LayerPaint::Raster(p) => {
            collect_prop(&p.raster_brightness_max, keys)
                && collect_prop(&p.raster_brightness_min, keys)
                && collect_prop(&p.raster_contrast, keys)
                && collect_prop(&p.raster_hue_rotate, keys)
                && collect_prop(&p.raster_opacity, keys)
                && collect_resampling(&p.raster_resampling, keys)
                && collect_prop(&p.raster_saturation, keys)
        }
        LayerPaint::Hillshade(p) => {
            collect_prop(&p.hillshade_illumination_direction, keys)
                && collect_prop(&p.hillshade_illumination_altitude, keys)
                && collect_prop(&p.hillshade_exaggeration, keys)
                && collect_prop(&p.hillshade_shadow_color, keys)
                && collect_prop(&p.hillshade_highlight_color, keys)
                && collect_prop(&p.hillshade_accent_color, keys)
                && collect_resampling(&p.hillshade_resampling, keys)
        }
        LayerPaint::ColorRelief(p) => {
            collect_prop(&p.color_relief_opacity, keys)
                && collect_prop(&p.color_relief_color, keys)
                && collect_resampling(&p.color_relief_resampling, keys)
        }
        _ => true,
    }
}

fn is_token_property(name: &str) -> bool {
    matches!(
        name,
        "text-field"
            | "icon-image"
            | "fill-pattern"
            | "line-pattern"
            | "fill-extrusion-pattern"
            | "background-pattern"
    )
}

fn collect_prop<T>(prop: &Option<StyleProperty<T>>, keys: &mut FxHashSet<String>) -> bool {
    match prop {
        Some(StyleProperty::Expression(expr)) => walk_expr(expr.expression(), keys),
        None | Some(StyleProperty::Constant(_) | StyleProperty::Unsupported(_)) => true,
    }
}

fn collect_translation(prop: &Option<Translation>, keys: &mut FxHashSet<String>) -> bool {
    match prop {
        Some(translation) => match translation.property() {
            StyleProperty::Expression(expr) => walk_expr(expr.expression(), keys),
            StyleProperty::Constant(_) | StyleProperty::Unsupported(_) => true,
        },
        None => true,
    }
}

fn collect_resampling(prop: &Option<RasterResampling>, keys: &mut FxHashSet<String>) -> bool {
    match prop {
        Some(RasterResampling::Expression(items)) => {
            items.iter().all(|item| walk_json(item, false, keys))
        }
        _ => true,
    }
}

fn collect_opt_json(
    value: &Option<serde_json::Value>,
    is_token_key: bool,
    keys: &mut FxHashSet<String>,
) -> bool {
    value
        .as_ref()
        .is_none_or(|v| walk_json(v, is_token_key, keys))
}

fn walk_expr(expr: &Expression, keys: &mut FxHashSet<String>) -> bool {
    match expr {
        Expression::Feature(FeatureProperty::Properties) => false,
        Expression::Get { key, object: None } | Expression::Has { key, object: None } => {
            match key.as_ref() {
                Expression::Literal(Value::String(k))
                | Expression::Folded {
                    value: Value::String(k),
                    ..
                } => {
                    keys.insert(k.clone());
                    true
                }
                _ => false,
            }
        }
        _ => {
            let mut ok = true;
            expr.for_each_child(&mut |child| {
                if ok && !walk_expr(child, keys) {
                    ok = false;
                }
            });
            ok
        }
    }
}

fn walk_json(value: &serde_json::Value, is_token_key: bool, keys: &mut FxHashSet<String>) -> bool {
    match value {
        serde_json::Value::String(s) if is_token_key && s.contains('{') => {
            let converted = convert_token_string(s);
            walk_json(&converted, false, keys)
        }
        serde_json::Value::Object(map) => {
            let prop = map.get("property");
            if let Some(prop_val) = prop {
                let Some(k) = prop_val.as_str() else {
                    return false;
                };
                keys.insert(k.to_owned());
            }
            if let Some(stops) = map.get("stops").and_then(serde_json::Value::as_array) {
                for stop in stops {
                    if let Some(output) = stop.get(1) {
                        if !walk_json(output, is_token_key && prop.is_none(), keys) {
                            return false;
                        }
                    }
                }
                return true;
            }
            let is_legacy_fn =
                prop.is_some() || map.contains_key("type") || map.contains_key("base");
            if !is_legacy_fn {
                for v in map.values() {
                    if !walk_json(v, false, keys) {
                        return false;
                    }
                }
            }
            true
        }
        serde_json::Value::Array(items) => {
            let op = items.first().and_then(serde_json::Value::as_str);
            match op {
                Some("properties") if items.len() == 1 => false,
                Some("literal") => true,
                Some("get" | "has") => match items.as_slice() {
                    [_, key] => {
                        let Some(k) = key.as_str() else {
                            return false;
                        };
                        keys.insert(k.to_owned());
                        true
                    }
                    [_, key, obj, ..] => walk_json(key, false, keys) && walk_json(obj, false, keys),
                    _ => true,
                },
                Some(_) => items
                    .get(1..)
                    .into_iter()
                    .flatten()
                    .all(|item| walk_json(item, false, keys)),
                None => items.iter().all(|item| walk_json(item, false, keys)),
            }
        }
        _ => true,
    }
}
