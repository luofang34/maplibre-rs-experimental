//! Classifies whether editing a [`StyleLayer`] requires worker re-tessellation (`redraw_tiles`)
//! or can be applied in-place on resident GPU buffers.

use serde::Serialize;
use serde_json::{json, Value};

use crate::{
    style::layer::{LayerPaint, StyleLayer, StyleProperty},
    vector::pattern::per_feature_pattern,
};

pub(super) fn push_unique(ids: &mut Vec<String>, id: &str) {
    if !ids.iter().any(|existing| existing == id) {
        ids.push(id.to_owned());
    }
}

/// Sets `document[section][name]`, or removes it for `null`.
pub(super) fn set_member(document: &mut Value, section: &str, name: &str, value: Value) {
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
    if !super::from_vector_tiles(after) {
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
    needs_paint_retessellation(before.paint.as_ref(), after.paint.as_ref())
}

fn needs_paint_retessellation(before: Option<&LayerPaint>, after: Option<&LayerPaint>) -> bool {
    match (before, after) {
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
                || a.fill_extrusion_translucent != b.fill_extrusion_translucent
                || a.fill_extrusion_pattern != b.fill_extrusion_pattern
        }
        (Some(LayerPaint::Heatmap(a)), Some(LayerPaint::Heatmap(b))) => {
            !same_prop(&a.heatmap_weight, &b.heatmap_weight)
                || ((a.radius_per_feature() || b.radius_per_feature())
                    && !same_prop(&a.heatmap_radius, &b.heatmap_radius))
        }
        (Some(LayerPaint::Symbol(_)), Some(LayerPaint::Symbol(_))) => false,
        _ => true,
    }
}
