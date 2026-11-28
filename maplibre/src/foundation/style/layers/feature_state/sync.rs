//! Synchronizes runtime `feature-state` into decoded GeoJSON and MVT feature property maps.

use std::{borrow::Cow, collections::BTreeMap, sync::Arc};

use rustc_hash::FxHashMap;
use serde_json::{Map, Value};

use super::{FeatureId, SourceFeatureState, STATE_ID_PROPERTY};
#[cfg(any(test, not(target_arch = "wasm32")))]
use crate::style::expression::json_reads_feature_state;
use crate::style::{
    expression::{
        is_internal_property, FeatureProperties, Value as ExprValue, FEATURE_STATE_PREFIX,
    },
    layer::StyleLayer,
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
    let Some(props) = ensure_geojson_properties_map(feature) else {
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

fn ensure_geojson_properties_map(feature: &mut Value) -> Option<&mut Map<String, Value>> {
    let obj = feature.as_object_mut()?;
    let props = obj
        .entry("properties")
        .or_insert_with(|| Value::Object(Map::new()));
    if !props.is_object() {
        *props = Value::Object(Map::new());
    }
    props.as_object_mut()
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
    if let Some(idx) = generated {
        return Some(idx.to_string());
    }
    feature
        .get("id")
        .and_then(FeatureId::from_json)
        .map(FeatureId::into_string)
}

/// Memoizes synchronized `Arc<FeatureProperties>` maps within a batch while keeping the original
/// `Arc` alive so its heap address cannot be freed and reused (`ABA`) by a subsequent message.
pub(crate) type FeatureStateArcMemo =
    FxHashMap<usize, (Arc<FeatureProperties>, Arc<FeatureProperties>, bool)>;

#[inline]
fn has_any_feature_state_key(properties: &FeatureProperties) -> bool {
    properties
        .keys()
        .any(|k| k.starts_with(FEATURE_STATE_PREFIX))
}

fn clear_feature_state_keys(properties: &mut FeatureProperties) -> bool {
    let had = has_any_feature_state_key(properties);
    if had {
        properties.retain(|k, _| !k.starts_with(FEATURE_STATE_PREFIX));
    }
    had
}

fn resolve_sync_state_id<'a>(
    properties: &'a FeatureProperties,
    raw_id: Option<u64>,
    promote_property: Option<&str>,
) -> Option<Cow<'a, str>> {
    if let Some(prop) = promote_property {
        properties
            .get(prop)
            .and_then(FeatureId::from_expr_value)
            .map(|id| Cow::Owned(id.into_string()))
    } else if let Some(ExprValue::String(existing)) = properties.get(STATE_ID_PROPERTY) {
        Some(Cow::Borrowed(existing.as_str()))
    } else {
        raw_id.map(|id| Cow::Owned(id.to_string()))
    }
}

#[inline]
fn is_stale_feature_state_key(key: &str, desired: Option<&BTreeMap<String, Value>>) -> bool {
    key.strip_prefix(FEATURE_STATE_PREFIX).is_some_and(|name| {
        desired
            .and_then(|map| map.get(name))
            .is_none_or(Value::is_null)
    })
}

fn apply_desired_feature_state(
    properties: &mut FeatureProperties,
    desired: &BTreeMap<String, Value>,
) -> bool {
    let mut changed = false;
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
    changed
}

fn desired_state_differs(
    properties: &FeatureProperties,
    desired: &BTreeMap<String, Value>,
) -> bool {
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
    false
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
    let non_empty_state = source_state.filter(|s| !s.is_empty());
    let Some(non_empty_state) = non_empty_state else {
        return clear_feature_state_keys(properties);
    };
    let Some(state_id) = resolve_sync_state_id(properties, raw_id, promote_property) else {
        return clear_feature_state_keys(properties);
    };
    let desired = non_empty_state.get(source_layer, state_id.as_ref());
    let mut changed = false;
    let stale: Vec<String> = properties
        .keys()
        .filter(|k| is_stale_feature_state_key(k, desired))
        .cloned()
        .collect();
    for key in stale {
        properties.remove(&key);
        changed = true;
    }
    if let Some(desired) = desired {
        changed |= apply_desired_feature_state(properties, desired);
    }
    changed
}

/// Synchronizes a shared `Arc<FeatureProperties>` against `source_state` without cloning unchanged
/// maps, reusing updated `Arc` allocations in `memo` (while keeping the original `Arc` alive so
/// its address cannot be reused across messages in the same batch).
pub(crate) fn sync_arc_properties_feature_state(
    properties: &mut Arc<FeatureProperties>,
    raw_id: Option<u64>,
    promote_property: Option<&str>,
    source_state: Option<&SourceFeatureState>,
    source_layer: Option<&str>,
    memo: &mut FeatureStateArcMemo,
) -> bool {
    let ptr = Arc::as_ptr(properties) as usize;
    if let Some((_, updated, changed)) = memo.get(&ptr) {
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
    let original = Arc::clone(properties);
    let mut next = (**properties).clone();
    let changed = sync_properties_feature_state(
        &mut next,
        raw_id,
        promote_property,
        source_state,
        source_layer,
    );
    let next_arc = Arc::new(next);
    memo.insert(ptr, (original, Arc::clone(&next_arc), changed));
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
    let non_empty_state = source_state.filter(|s| !s.is_empty());
    let Some(non_empty_state) = non_empty_state else {
        return has_any_feature_state_key(properties);
    };
    let Some(state_id) = resolve_sync_state_id(properties, raw_id, promote_property) else {
        return has_any_feature_state_key(properties);
    };
    let desired = non_empty_state.get(source_layer, state_id.as_ref());
    if properties
        .keys()
        .any(|k| is_stale_feature_state_key(k, desired))
    {
        return true;
    }
    desired.is_some_and(|desired| desired_state_differs(properties, desired))
}

pub(crate) use crate::style::layer::classify::layer_paint_reads_feature_state;

/// Returns `true` when `layer` reads `["feature-state", ...]` in a property that affects
/// tessellated geometry or layout rather than GPU-evaluated feature colors.
pub(crate) fn layer_geometry_reads_feature_state(layer: &StyleLayer) -> bool {
    if let Some(flags) = layer.input_flags {
        return (flags & crate::style::layer::FLAG_GEOM_READS_FEATURE_STATE) != 0;
    }
    #[cfg(not(any(test, not(target_arch = "wasm32"))))]
    {
        false
    }
    #[cfg(any(test, not(target_arch = "wasm32")))]
    {
        use crate::style::layer::LayerPaint;
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
