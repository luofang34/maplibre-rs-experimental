//! Typed MVT feature property decoding, scratch reuse, and bounded memoization.

use std::sync::Arc;

use geozero::mvt::tile;
use rustc_hash::FxHashMap;

use crate::{
    style::expression::{empty_feature_properties, FeatureProperties, Value},
    vector::tessellation::feature_paint::FeatureStateSyncContext,
};

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
        let value = if let Some(text) = &value.string_value {
            Value::String(text.clone())
        } else if let Some(number) = value.float_value {
            Value::Number(f64::from(number))
        } else if let Some(number) = value.double_value {
            Value::Number(number)
        } else if let Some(number) = value.int_value {
            Value::Number(number as f64)
        } else if let Some(number) = value.uint_value {
            Value::Number(number as f64)
        } else if let Some(number) = value.sint_value {
            Value::Number(number as f64)
        } else if let Some(flag) = value.bool_value {
            Value::Bool(flag)
        } else {
            continue;
        };
        properties.insert(key.clone(), value);
    }
}

/// Pre-decodes every feature's typed properties in `layer` into shared [`Arc`] maps unless
/// `state_ctx` is absent and distinct non-empty tag combinations exceed `max_unique` (returning
/// `None` so high-cardinality layers stream per-feature with a single scratch map).
pub(in crate::vector::process_vector) fn decode_layer_properties_bounded(
    layer: &tile::Layer,
    state_ctx: Option<&FeatureStateSyncContext>,
    max_unique: usize,
) -> Option<Vec<Arc<FeatureProperties>>> {
    let mut out = Vec::with_capacity(layer.features.len());
    if let Some(ctx) = state_ctx {
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
        return Some(out);
    }

    let empty = empty_feature_properties();
    let mut memo: FxHashMap<&[u32], Arc<FeatureProperties>> = FxHashMap::default();
    for feature in &layer.features {
        if feature.tags.is_empty() {
            out.push(Arc::clone(empty));
            continue;
        }
        if let Some(existing) = memo.get(feature.tags.as_slice()) {
            out.push(Arc::clone(existing));
            continue;
        }
        if memo.len() >= max_unique || !tags_are_valid(layer, &feature.tags) {
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

/// Pre-decodes every feature's typed properties in `layer` once into `Arc<FeatureProperties>`
/// (including active `feature-state` entries), memoizing identical MVT tag slices so repeated
/// attribute combinations share a single heap map. Returns `None` if any feature's tag table is
/// malformed so callers can fall back to `geozero::mvt::process` error reporting.
pub(in crate::vector::process_vector) fn decode_layer_properties(
    layer: &tile::Layer,
    state_ctx: Option<&FeatureStateSyncContext>,
) -> Option<Vec<Arc<FeatureProperties>>> {
    decode_layer_properties_bounded(layer, state_ctx, usize::MAX)
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
