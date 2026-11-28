//! Classifies whether editing a [`StyleLayer`] requires worker re-tessellation (`redraw_tiles`)
//! or can be applied in-place on resident GPU buffers.

use serde::Serialize;
use serde_json::{json, Value};

use crate::style::layer::{
    LayerPaint, PatternName, StyleLayer, StyleProperty, FLAG_READS_FEATURE_STATE,
};

pub(crate) fn push_unique(ids: &mut Vec<String>, id: &str) {
    if !ids.iter().any(|existing| existing == id) {
        ids.push(id.to_owned());
    }
}

/// Sets `document[section][name]`, or removes it for `null`.
pub(crate) fn set_member(document: &mut Value, section: &str, name: &str, value: Value) {
    let Some(map) = document.as_object_mut() else {
        return;
    };
    if value.is_null() {
        if let Some(Value::Object(members)) = map.get_mut(section) {
            members.remove(name);
        }
        return;
    }
    let members = map.entry(section).or_insert_with(|| json!({}));
    if let Some(members) = members.as_object_mut() {
        members.insert(name.to_owned(), value);
    }
}

fn same_prop<T: Serialize>(a: &T, b: &T) -> bool {
    serde_json::to_value(a).ok() == serde_json::to_value(b).ok()
}

fn is_data_driven<T: crate::style::property::PropertyValue>(
    prop: Option<&StyleProperty<T>>,
) -> bool {
    prop.is_some_and(|p| !p.is_feature_constant())
}

/// Returns `true` only when `before -> after` changes geometry, filtering, or layout baked by
/// workers rather than layer order, visibility hiding, zoom bounds, or GPU paint buffers.
pub(crate) fn needs_tile_redraw(before: &StyleLayer, after: &StyleLayer) -> bool {
    if !crate::style::layer::from_vector_tiles(after) {
        return false;
    }
    // Hidden layers are omitted from worker tile requests, so unhiding needs tessellation.
    if before.is_hidden() && !after.is_hidden() {
        return true;
    }
    // Hiding a visible layer updates GPU pool visibility in-place.
    if after.is_hidden() {
        return false;
    }
    if before.type_ != after.type_
        || before.source != after.source
        || before.source_layer != after.source_layer
        || before.filter != after.filter
        || before.metadata != after.metadata
    {
        return true;
    }
    let Ok(mut before_maps) = before.property_maps() else {
        return true;
    };
    let Ok(mut after_maps) = after.property_maps() else {
        return true;
    };
    before_maps.layout.remove("visibility");
    after_maps.layout.remove("visibility");
    if before_maps.layout != after_maps.layout {
        return true;
    }
    if !layer_retains_feature_paint(before) && layer_retains_feature_paint(after) {
        return true;
    }
    needs_paint_retessellation(before, after)
}

fn needs_paint_retessellation(before: &StyleLayer, after: &StyleLayer) -> bool {
    match (before.paint.as_ref(), after.paint.as_ref()) {
        (None, None) => false,
        (Some(bp @ LayerPaint::Fill(a)), Some(ap @ LayerPaint::Fill(b))) => {
            a.outline_color().is_some() != b.outline_color().is_some()
                || ((per_feature_pattern(bp).is_some() || per_feature_pattern(ap).is_some())
                    && a.fill_pattern != b.fill_pattern)
        }
        (Some(bp @ LayerPaint::Line(a)), Some(ap @ LayerPaint::Line(b))) => {
            let per_feature_stroke = is_data_driven(a.line_width.as_ref())
                || is_data_driven(b.line_width.as_ref())
                || is_data_driven(a.line_offset.as_ref())
                || is_data_driven(b.line_offset.as_ref())
                || is_data_driven(a.line_gap_width.as_ref())
                || is_data_driven(b.line_gap_width.as_ref())
                || is_data_driven(a.line_blur.as_ref())
                || is_data_driven(b.line_blur.as_ref());
            (per_feature_stroke
                && (!same_prop(&a.line_width, &b.line_width)
                    || !same_prop(&a.line_offset, &b.line_offset)
                    || !same_prop(&a.line_gap_width, &b.line_gap_width)
                    || !same_prop(&a.line_blur, &b.line_blur)))
                || a.line_gradient.is_some() != b.line_gradient.is_some()
                || a.line_dasharray.is_some() != b.line_dasharray.is_some()
                || a.line_pattern.is_some() != b.line_pattern.is_some()
                || ((per_feature_pattern(bp).is_some() || per_feature_pattern(ap).is_some())
                    && a.line_pattern != b.line_pattern)
                || !same_prop(&a.line_elevation, &b.line_elevation)
                || !same_prop(&a.line_elevation_datum, &b.line_elevation_datum)
        }
        (Some(LayerPaint::Circle(a)), Some(LayerPaint::Circle(b))) => {
            !same_prop(&a.circle_radius, &b.circle_radius)
                || !same_prop(&a.circle_stroke_width, &b.circle_stroke_width)
                || a.circle_pitch_alignment != b.circle_pitch_alignment
                || ((is_data_driven(a.circle_blur.as_ref())
                    || is_data_driven(b.circle_blur.as_ref()))
                    && !same_prop(&a.circle_blur, &b.circle_blur))
                || ((is_data_driven(a.circle_stroke_opacity.as_ref())
                    || is_data_driven(b.circle_stroke_opacity.as_ref()))
                    && !same_prop(&a.circle_stroke_opacity, &b.circle_stroke_opacity))
        }
        (Some(LayerPaint::FillExtrusion(a)), Some(LayerPaint::FillExtrusion(b))) => {
            !same_prop(&a.fill_extrusion_height, &b.fill_extrusion_height)
                || !same_prop(&a.fill_extrusion_base, &b.fill_extrusion_base)
                || !same_prop(&a.fill_extrusion_base_datum, &b.fill_extrusion_base_datum)
                || !same_prop(
                    &a.fill_extrusion_height_datum,
                    &b.fill_extrusion_height_datum,
                )
                || !same_prop(&a.fill_extrusion_part, &b.fill_extrusion_part)
                || !same_prop(
                    &a.fill_extrusion_rounded_corner_distance,
                    &b.fill_extrusion_rounded_corner_distance,
                )
                || a.fill_extrusion_translucent != b.fill_extrusion_translucent
                || a.fill_extrusion_pattern != b.fill_extrusion_pattern
        }
        (Some(LayerPaint::Heatmap(a)), Some(LayerPaint::Heatmap(b))) => {
            !same_prop(&a.heatmap_weight, &b.heatmap_weight)
                || ((a.radius_per_feature() || b.radius_per_feature())
                    && !same_prop(&a.heatmap_radius, &b.heatmap_radius))
        }
        (Some(LayerPaint::Symbol(_)), Some(LayerPaint::Symbol(_))) => {
            match (
                crate::style::layer::symbol_runtime_referenced_keys(before, None),
                crate::style::layer::symbol_runtime_referenced_keys(after, None),
            ) {
                (None, _) => false,
                (Some(_), None) => true,
                (Some(prev), Some(next)) => !next.is_subset(&prev),
            }
        }
        _ => true,
    }
}

/// The sort-key property a layer type accepts, if any.
pub fn sort_key_name(layer_type: &str) -> Option<&'static str> {
    match layer_type {
        "fill" => Some("fill-sort-key"),
        "line" => Some("line-sort-key"),
        "circle" => Some("circle-sort-key"),
        _ => None,
    }
}

/// The pattern property of a fill, extrusion or line layer when its image varies by feature.
pub(crate) fn per_feature_pattern(paint: &LayerPaint) -> Option<StyleProperty<PatternName>> {
    let value = match paint {
        LayerPaint::Fill(fill) => fill.fill_pattern.as_ref()?,
        LayerPaint::FillExtrusion(extrusion) => extrusion.fill_extrusion_pattern.as_ref()?,
        LayerPaint::Line(line) => line.line_pattern.as_ref()?,
        _ => return None,
    };
    let property = StyleProperty::<PatternName>::parse(value);
    (!property.is_feature_constant()).then_some(property)
}

/// Returns `true` when any paint property on `layer` reads `["feature-state", ...]`.
pub(crate) fn layer_paint_reads_feature_state(layer: &StyleLayer) -> bool {
    if let Some(flags) = layer.input_flags {
        return (flags & FLAG_READS_FEATURE_STATE) != 0;
    }
    #[cfg(not(any(test, not(target_arch = "wasm32"))))]
    {
        false
    }
    #[cfg(any(test, not(target_arch = "wasm32")))]
    {
        use crate::style::expression::json_reads_feature_state;
        if layer
            .unrecognized
            .paint
            .values()
            .any(json_reads_feature_state)
        {
            return true;
        }
        layer
            .paint
            .as_ref()
            .and_then(|paint| serde_json::to_value(paint).ok())
            .is_some_and(|json| json_reads_feature_state(&json))
    }
}

/// Returns `true` when `layer` is a non-symbol vector layer whose GPU feature paint depends on
/// per-feature properties or runtime `feature-state`, requiring retained feature entries.
pub(crate) fn layer_retains_feature_paint(layer: &StyleLayer) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        layer_paint_reads_feature_state(layer)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if layer_paint_reads_feature_state(layer) {
            return true;
        }
        let Some(paint) = layer.paint.as_ref() else {
            return false;
        };
        layer_paint_varies_by_feature(paint)
    }
}

/// Returns `true` when any GPU-evaluated paint property in `paint` varies by feature.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn layer_paint_varies_by_feature(paint: &LayerPaint) -> bool {
    if paint
        .opacity()
        .as_ref()
        .is_some_and(|prop| !prop.is_feature_constant())
        || per_feature_pattern(paint).is_some()
    {
        return true;
    }
    match paint {
        LayerPaint::Fill(fill) => {
            fill.fill_color
                .as_ref()
                .is_some_and(|prop| !prop.is_feature_constant())
                || fill
                    .outline_color()
                    .as_ref()
                    .is_some_and(|prop| !prop.is_feature_constant())
        }
        LayerPaint::Line(line) => line
            .line_color
            .as_ref()
            .is_some_and(|prop| !prop.is_feature_constant()),
        LayerPaint::Circle(circle) => {
            circle
                .circle_color
                .as_ref()
                .is_some_and(|prop| !prop.is_feature_constant())
                || circle.stroke_color_per_feature().is_some()
        }
        LayerPaint::FillExtrusion(ext) => ext
            .fill_extrusion_color
            .as_ref()
            .is_some_and(|prop| !prop.is_feature_constant()),
        _ => false,
    }
}
