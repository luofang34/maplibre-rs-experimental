//! Synchronizes runtime `feature-state` into decoded GeoJSON and MVT feature property maps.

use std::{borrow::Cow, collections::BTreeMap, sync::Arc};

use rustc_hash::FxHashMap;
use serde_json::{Map, Value};

use super::{FeatureId, SourceFeatureState, STATE_ID_PROPERTY};
use crate::style::{
    expression::{
        is_internal_property, FeatureProperties, Value as ExprValue, FEATURE_STATE_PREFIX,
    },
    layer::{LayerPaint, StyleLayer},
    source::Source,
    Style,
};

impl Style {
    /// Annotates an inline GeoJSON value with [`STATE_ID_PROPERTY`] and any active
    /// `feature-state` properties for `source_name`.
    pub fn apply_feature_state_to_geojson<'a>(
        &self,
        source_name: &str,
        geojson: &'a Value,
    ) -> Cow<'a, Value> {
        let (promote, generate_id) = match self.sources.get(source_name) {
            Some(Source::GeoJson(src)) => (
                src.promote_id.as_ref().and_then(|p| p.property_for(None)),
                src.generate_id,
            ),
            _ => (None, false),
        };
        let source_state = self.feature_state.source(source_name);
        let mut cloned = geojson.clone();
        match cloned.get("type").and_then(Value::as_str) {
            Some("FeatureCollection") => {
                if let Some(features) = cloned.get_mut("features").and_then(Value::as_array_mut) {
                    for (idx, feature) in features.iter_mut().enumerate() {
                        annotate_geojson_feature(
                            feature,
                            promote,
                            generate_id.then_some(idx),
                            source_state,
                        );
                    }
                }
                Cow::Owned(cloned)
            }
            Some("Feature") => {
                annotate_geojson_feature(
                    &mut cloned,
                    promote,
                    generate_id.then_some(0),
                    source_state,
                );
                Cow::Owned(cloned)
            }
            _ => Cow::Borrowed(geojson),
        }
    }
}

fn annotate_geojson_feature(
    feature: &mut Value,
    promote: Option<&str>,
    generated: Option<usize>,
    source_state: Option<&SourceFeatureState>,
) {
    let Some(state_id) = resolve_geojson_state_id(feature, promote, generated) else {
        return;
    };
    let Some(obj) = feature.as_object_mut() else {
        return;
    };
    let props = obj
        .entry("properties")
        .or_insert_with(|| Value::Object(Map::new()));
    if !props.is_object() {
        *props = Value::Object(Map::new());
    }
    let Some(props) = props.as_object_mut() else {
        return;
    };
    props.insert(
        STATE_ID_PROPERTY.to_owned(),
        Value::String(state_id.clone()),
    );
    if let Some(values) = source_state.and_then(|s| s.get(None, &state_id)) {
        for (k, v) in values {
            if !v.is_null() {
                props.insert(format!("{FEATURE_STATE_PREFIX}{k}"), v.clone());
            }
        }
    }
}

/// Resolves the canonical state ID string of a GeoJSON feature according to `promoteId` and
/// `generateId`.
pub(crate) fn resolve_geojson_state_id(
    feature: &Value,
    promote: Option<&str>,
    generated: Option<usize>,
) -> Option<String> {
    if let Some(prop) = promote {
        return feature
            .get("properties")
            .and_then(|p| p.get(prop))
            .and_then(FeatureId::from_json)
            .map(FeatureId::into_string);
    }
    feature
        .get("id")
        .and_then(FeatureId::from_json)
        .map(FeatureId::into_string)
        .or_else(|| generated.map(|idx| idx.to_string()))
}

/// Resolves a feature's canonical state ID and synchronizes its `\u{1}state:*` entries in
/// `properties`, returning `true` when any `feature-state` value changed.
pub(crate) fn sync_properties_feature_state(
    properties: &mut FeatureProperties,
    raw_id: Option<u64>,
    promote_property: Option<&str>,
    source_state: Option<&SourceFeatureState>,
    source_layer: Option<&str>,
) -> bool {
    let resolved = if let Some(prop) = promote_property {
        properties
            .get(prop)
            .and_then(FeatureId::from_expr_value)
            .map(FeatureId::into_string)
    } else if let Some(ExprValue::String(existing)) = properties.get(STATE_ID_PROPERTY) {
        Some(existing.clone())
    } else {
        raw_id.map(|id| id.to_string())
    };
    let Some(state_id) = resolved else {
        let had = properties
            .keys()
            .any(|k| k.starts_with(FEATURE_STATE_PREFIX));
        if had {
            properties.retain(|k, _| !k.starts_with(FEATURE_STATE_PREFIX));
        }
        return had;
    };
    if !matches!(properties.get(STATE_ID_PROPERTY), Some(ExprValue::String(s)) if s == &state_id) {
        properties.insert(
            STATE_ID_PROPERTY.to_owned(),
            ExprValue::String(state_id.clone()),
        );
    }
    let desired = source_state.and_then(|s| s.get(source_layer, &state_id));
    let mut changed = false;
    let stale: Vec<String> = properties
        .keys()
        .filter(|k| {
            k.strip_prefix(FEATURE_STATE_PREFIX).is_some_and(|name| {
                desired
                    .and_then(|map| map.get(name))
                    .is_none_or(Value::is_null)
            })
        })
        .cloned()
        .collect();
    for key in stale {
        properties.remove(&key);
        changed = true;
    }
    if let Some(desired) = desired {
        for (k, v) in desired {
            if v.is_null() {
                continue;
            }
            let expr_val = ExprValue::from_json(v);
            let full_key = format!("{FEATURE_STATE_PREFIX}{k}");
            if properties.get(&full_key) != Some(&expr_val) {
                properties.insert(full_key, expr_val);
                changed = true;
            }
        }
    }
    changed
}

/// Synchronizes a shared `Arc<FeatureProperties>` against `source_state` without cloning unchanged
/// maps, reusing updated `Arc` allocations in `memo` so shared feature parts and style layers keep
/// identical `Arc` pointers.
pub(crate) fn sync_arc_properties_feature_state(
    properties: &mut Arc<FeatureProperties>,
    raw_id: Option<u64>,
    promote_property: Option<&str>,
    source_state: Option<&SourceFeatureState>,
    source_layer: Option<&str>,
    memo: &mut FxHashMap<usize, (Arc<FeatureProperties>, bool)>,
) -> bool {
    let ptr = Arc::as_ptr(properties) as usize;
    if let Some((updated, changed)) = memo.get(&ptr) {
        *properties = Arc::clone(updated);
        return *changed;
    }
    if !properties_need_feature_state_sync(
        properties,
        raw_id,
        promote_property,
        source_state,
        source_layer,
    ) {
        return false;
    }
    let mut next = (**properties).clone();
    let changed = sync_properties_feature_state(
        &mut next,
        raw_id,
        promote_property,
        source_state,
        source_layer,
    );
    let next_arc = Arc::new(next);
    memo.insert(ptr, (Arc::clone(&next_arc), changed));
    *properties = next_arc;
    changed
}

fn properties_need_feature_state_sync(
    properties: &FeatureProperties,
    raw_id: Option<u64>,
    promote_property: Option<&str>,
    source_state: Option<&SourceFeatureState>,
    source_layer: Option<&str>,
) -> bool {
    let resolved: Option<Cow<'_, str>> = if let Some(prop) = promote_property {
        properties
            .get(prop)
            .and_then(FeatureId::from_expr_value)
            .map(|id| Cow::Owned(id.into_string()))
    } else if let Some(ExprValue::String(existing)) = properties.get(STATE_ID_PROPERTY) {
        Some(Cow::Borrowed(existing.as_str()))
    } else {
        raw_id.map(|id| Cow::Owned(id.to_string()))
    };
    let Some(state_id) = resolved else {
        return properties
            .keys()
            .any(|k| k.starts_with(FEATURE_STATE_PREFIX));
    };
    if !matches!(properties.get(STATE_ID_PROPERTY), Some(ExprValue::String(s)) if s == state_id.as_ref())
    {
        return true;
    }
    let desired = source_state.and_then(|s| s.get(source_layer, state_id.as_ref()));
    if properties.keys().any(|k| {
        k.strip_prefix(FEATURE_STATE_PREFIX).is_some_and(|name| {
            desired
                .and_then(|map| map.get(name))
                .is_none_or(Value::is_null)
        })
    }) {
        return true;
    }
    if let Some(desired) = desired {
        for (k, v) in desired {
            if v.is_null() {
                continue;
            }
            let expr_val = ExprValue::from_json(v);
            let full_key = format!("{FEATURE_STATE_PREFIX}{k}");
            if properties.get(&full_key) != Some(&expr_val) {
                return true;
            }
        }
    }
    false
}

/// Returns `true` when any paint property on `layer` reads `["feature-state", ...]`.
pub(crate) fn layer_paint_reads_feature_state(layer: &StyleLayer) -> bool {
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

/// Returns `true` when `layer` reads `["feature-state", ...]` in a property that affects
/// tessellated geometry or layout rather than GPU-evaluated feature colors.
pub(crate) fn layer_geometry_reads_feature_state(layer: &StyleLayer) -> bool {
    if layer.filter.as_ref().is_some_and(json_reads_feature_state)
        || layer
            .unrecognized
            .layout
            .values()
            .any(json_reads_feature_state)
    {
        return true;
    }
    let Some(paint) = &layer.paint else {
        return false;
    };
    let Ok(Value::Object(mut fields)) = serde_json::to_value(paint) else {
        return false;
    };
    if let Some(Value::Object(mut inner)) = fields.remove("paint") {
        // Remove properties whose per-feature values are evaluated on the CPU/GPU metadata path
        // without re-tessellating vertices.
        match paint {
            LayerPaint::Fill(_) => {
                inner.remove("fill-color");
                inner.remove("fill-opacity");
                inner.remove("fill-outline-color");
            }
            LayerPaint::Line(_) => {
                inner.remove("line-color");
                inner.remove("line-opacity");
            }
            LayerPaint::Circle(_) => {
                inner.remove("circle-color");
                inner.remove("circle-opacity");
                inner.remove("circle-stroke-color");
            }
            LayerPaint::FillExtrusion(_) => {
                inner.remove("fill-extrusion-color");
                inner.remove("fill-extrusion-opacity");
            }
            LayerPaint::Symbol(_) => {
                for key in [
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
                ] {
                    inner.remove(key);
                }
            }
            _ => {}
        }
        return inner.values().any(json_reads_feature_state);
    }
    false
}

fn json_reads_feature_state(value: &Value) -> bool {
    match value {
        Value::Array(items) => {
            matches!(items.first(), Some(Value::String(op)) if op == "feature-state")
                || items.iter().any(json_reads_feature_state)
        }
        Value::Object(map) => map.values().any(json_reads_feature_state),
        _ => false,
    }
}

pub(crate) fn extract_source_properties(properties: &FeatureProperties) -> BTreeMap<String, Value> {
    properties
        .iter()
        .filter(|(key, _)| !is_internal_property(key))
        .map(|(key, value)| (key.clone(), value.to_json()))
        .collect()
}

pub(crate) fn extract_feature_state(properties: &FeatureProperties) -> BTreeMap<String, Value> {
    properties
        .iter()
        .filter_map(|(key, value)| {
            key.strip_prefix(FEATURE_STATE_PREFIX)
                .map(|name| (name.to_owned(), value.to_json()))
        })
        .collect()
}
