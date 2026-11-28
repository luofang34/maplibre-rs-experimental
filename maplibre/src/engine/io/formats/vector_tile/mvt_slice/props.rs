//! Typed MVT feature property decoding, scratch reuse, and bounded memoization.

use std::sync::Arc;

use geozero::mvt::tile;
use rustc_hash::FxHashMap;

use crate::style::{
    expression::{empty_feature_properties, FeatureProperties, Value},
    feature_state::FeatureStateSyncContext,
};

fn mvt_value_to_property(value: &tile::Value) -> Option<Value> {
    if let Some(text) = &value.string_value {
        Some(Value::String(text.clone()))
    } else if let Some(number) = value.float_value {
        Some(Value::Number(f64::from(number)))
    } else if let Some(number) = value.double_value {
        Some(Value::Number(number))
    } else if let Some(number) = value.int_value {
        Some(Value::Number(number as f64))
    } else if let Some(number) = value.uint_value {
        Some(Value::Number(number as f64))
    } else if let Some(number) = value.sint_value {
        Some(Value::Number(number as f64))
    } else {
        value.bool_value.map(Value::Bool)
    }
}

/// Populates `properties` with typed filter values decoded from `feature.tags`, clearing any
/// previous entries while reusing the map allocation.
pub(crate) fn fill_feature_properties(
    layer: &tile::Layer,
    feature: &tile::Feature,
    properties: &mut FeatureProperties,
) {
    properties.clear();
    for pair in feature.tags.chunks(2) {
        let [key_index, value_index] = pair else {
            continue;
        };
        let (Some(key), Some(value)) = (
            layer.keys.get(*key_index as usize),
            layer.values.get(*value_index as usize),
        ) else {
            continue;
        };
        if let Some(converted) = mvt_value_to_property(value) {
            properties.insert(key.clone(), converted);
        }
    }
}

fn decode_layer_with_state(
    layer: &tile::Layer,
    ctx: &FeatureStateSyncContext,
) -> Option<Vec<Arc<FeatureProperties>>> {
    let mut out = Vec::with_capacity(layer.features.len());
    for feature in &layer.features {
        if !tags_are_valid(layer, &feature.tags) {
            return None;
        }
        let mut properties = FeatureProperties::default();
        fill_feature_properties(layer, feature, &mut properties);
        crate::style::feature_state::sync_properties_feature_state(
            &mut properties,
            feature.id,
            ctx.promote_property.as_deref(),
            ctx.source_state.as_deref(),
            ctx.source_layer.as_deref(),
        );
        if properties.is_empty() {
            out.push(Arc::clone(empty_feature_properties()));
        } else {
            out.push(Arc::new(properties));
        }
    }
    Some(out)
}

fn decode_layer_memoized(
    layer: &tile::Layer,
    max_unique: Option<usize>,
) -> Option<Vec<Arc<FeatureProperties>>> {
    let empty = empty_feature_properties();
    let mut out = Vec::with_capacity(layer.features.len());
    let mut memo: FxHashMap<&[u32], Arc<FeatureProperties>> = FxHashMap::default();
    let limit = max_unique.unwrap_or(usize::MAX);
    for feature in &layer.features {
        if feature.tags.is_empty() {
            out.push(Arc::clone(empty));
            continue;
        }
        if let Some(existing) = memo.get(feature.tags.as_slice()) {
            out.push(Arc::clone(existing));
            continue;
        }
        if memo.len() >= limit || !tags_are_valid(layer, &feature.tags) {
            return None;
        }
        let mut properties = FeatureProperties::default();
        fill_feature_properties(layer, feature, &mut properties);
        let arc = if properties.is_empty() {
            Arc::clone(empty)
        } else {
            Arc::new(properties)
        };
        memo.insert(feature.tags.as_slice(), Arc::clone(&arc));
        out.push(arc);
    }
    Some(out)
}

/// Pre-decodes every feature's typed properties in `layer` into shared [`Arc`] maps (including
/// active `feature-state` entries), memoizing identical MVT tag slices unless `state_ctx` is
/// absent and distinct non-empty tag combinations reach `max_unique`.
pub(crate) fn decode_layer_properties(
    layer: &tile::Layer,
    state_ctx: Option<&FeatureStateSyncContext>,
    max_unique: Option<usize>,
) -> Option<Vec<Arc<FeatureProperties>>> {
    match state_ctx {
        Some(ctx) => decode_layer_with_state(layer, ctx),
        None => decode_layer_memoized(layer, max_unique),
    }
}

fn tags_are_valid(layer: &tile::Layer, tags: &[u32]) -> bool {
    let (pairs, remainder) = tags.as_chunks::<2>();
    if !remainder.is_empty() {
        return false;
    }
    for &[key_idx, val_idx] in pairs {
        if layer.keys.get(key_idx as usize).is_none() {
            return false;
        }
        let Some(val) = layer.values.get(val_idx as usize) else {
            return false;
        };
        let has_val = val.string_value.is_some()
            || val.float_value.is_some()
            || val.double_value.is_some()
            || val.int_value.is_some()
            || val.uint_value.is_some()
            || val.sint_value.is_some()
            || val.bool_value.is_some();
        if !has_val {
            return false;
        }
    }
    true
}
