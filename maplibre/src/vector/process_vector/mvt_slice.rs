//! Zero-clone MVT layer iteration, feature-index filtering, and header construction.

use std::sync::Arc;

use geozero::{
    error::Result as GeoResult,
    mvt::{process_geom, tile, MvtError},
    ColumnValue, FeatureProcessor, GeozeroGeometry, ProcessorSink,
};

use super::feature_properties;
use crate::{
    style::{
        expression::{FeatureProperties, Value},
        filter::{FeatureContext, Filter, GeometryType},
    },
    vector::tessellation::feature_paint::FeatureStateSyncContext,
};

/// Pre-decodes every feature's typed properties in `layer` once into `Arc<FeatureProperties>`
/// (including active `feature-state` entries), returning `None` if any feature's tag table is
/// malformed so callers can fall back to `geozero::mvt::process` error reporting.
pub(super) fn decode_layer_properties(
    layer: &tile::Layer,
    state_ctx: Option<&FeatureStateSyncContext>,
) -> Option<Vec<Arc<FeatureProperties>>> {
    let mut out = Vec::with_capacity(layer.features.len());
    for feature in &layer.features {
        if !tags_are_valid(layer, &feature.tags) {
            return None;
        }
        let mut properties = feature_properties(layer, feature);
        if let Some(ctx) = state_ctx {
            crate::style::feature_state::sync_properties_feature_state(
                &mut properties,
                feature.id,
                ctx.promote_property.as_deref(),
                ctx.source_state.as_deref(),
                ctx.source_layer.as_deref(),
            );
        }
        out.push(Arc::new(properties));
    }
    Some(out)
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

/// Evaluates `filter` against `layer.features` without cloning or mutating `layer`, reusing
/// `shared_props` when available and returning matching feature indices in layer order.
pub(super) fn select_filtered_features(
    layer: &tile::Layer,
    filter: &Filter,
    zoom: f64,
    state_ctx: Option<&FeatureStateSyncContext>,
    shared_props: Option<&[Arc<FeatureProperties>]>,
) -> Vec<usize> {
    let mut selected = Vec::new();
    for (index, feature) in layer.features.iter().enumerate() {
        let owned;
        let props_ref: &FeatureProperties =
            if let Some(shared) = shared_props.and_then(|all| all.get(index)) {
                shared.as_ref()
            } else {
                let mut properties = feature_properties(layer, feature);
                if let Some(ctx) = state_ctx {
                    crate::style::feature_state::sync_properties_feature_state(
                        &mut properties,
                        feature.id,
                        ctx.promote_property.as_deref(),
                        ctx.source_state.as_deref(),
                        ctx.source_layer.as_deref(),
                    );
                }
                owned = properties;
                &owned
            };
        if filter.evaluate(&FeatureContext {
            properties: props_ref,
            geometry_type: GeometryType::from_mvt(feature.r#type.unwrap_or_default()),
            id: feature.id.map(|id| Value::Number(id as f64)),
            zoom,
        }) {
            selected.push(index);
        }
    }
    selected
}

/// Collects shared `Arc<FeatureProperties>` handles matching `selected` (or all features when
/// `selected` is `None`), or an empty vector when `shared` is `None`.
pub(super) fn shared_props_for_slice(
    shared: Option<&[Arc<FeatureProperties>]>,
    selected: Option<&[usize]>,
) -> Vec<Arc<FeatureProperties>> {
    let Some(all) = shared else {
        return Vec::new();
    };
    match selected {
        Some(indices) => indices
            .iter()
            .filter_map(|&idx| all.get(idx).cloned())
            .collect(),
        None => all.to_vec(),
    }
}

/// Collects source feature IDs for either `selected` indices or all features in `layer`.
pub(super) fn source_ids_for_slice(
    layer: &tile::Layer,
    selected: Option<&[usize]>,
) -> Vec<Option<u64>> {
    match selected {
        Some(indices) => indices
            .iter()
            .map(|&idx| layer.features.get(idx).and_then(|feature| feature.id))
            .collect(),
        None => layer.features.iter().map(|feature| feature.id).collect(),
    }
}

/// Streams `layer` (or only `selected` feature indices) into `processor` by shared reference
/// without cloning `tile::Layer` or its features. When `skip_properties` is `true`, tag-table
/// decoding is bypassed because the processor already holds shared `Arc<FeatureProperties>`.
pub(super) fn process_mvt_slice(
    layer: &tile::Layer,
    selected: Option<&[usize]>,
    skip_properties: bool,
    processor: &mut impl FeatureProcessor,
) -> GeoResult<()> {
    match (selected, skip_properties) {
        (None, false) => geozero::mvt::process(layer, processor),
        (None, true) => {
            processor.dataset_begin(Some(&layer.name))?;
            for (out_idx, feature) in layer.features.iter().enumerate() {
                processor.feature_begin(out_idx as u64)?;
                processor.geometry_begin()?;
                process_geom(feature, processor)?;
                processor.geometry_end()?;
                processor.feature_end(out_idx as u64)?;
            }
            processor.dataset_end()
        }
        (Some(indices), _) => {
            processor.dataset_begin(Some(&layer.name))?;
            for (out_idx, &feat_idx) in indices.iter().enumerate() {
                let Some(feature) = layer.features.get(feat_idx) else {
                    continue;
                };
                processor.feature_begin(out_idx as u64)?;
                if !skip_properties {
                    process_feature_properties(layer, feature, processor)?;
                }
                processor.geometry_begin()?;
                process_geom(feature, processor)?;
                processor.geometry_end()?;
                processor.feature_end(out_idx as u64)?;
            }
            processor.dataset_end()
        }
    }
}

fn process_feature_properties(
    layer: &tile::Layer,
    feature: &tile::Feature,
    processor: &mut impl FeatureProcessor,
) -> GeoResult<()> {
    processor.properties_begin()?;
    for (i, pair) in feature.tags.chunks(2).enumerate() {
        let [key_idx, value_idx] = pair else {
            return Err(MvtError::InvalidFeatureTagsLength(feature.tags.len()).into());
        };
        let key = layer
            .keys
            .get(*key_idx as usize)
            .ok_or(MvtError::InvalidKeyIndex(*key_idx))?;
        let value = layer
            .values
            .get(*value_idx as usize)
            .ok_or(MvtError::InvalidValueIndex(*value_idx))?;

        if let Some(ref v) = value.string_value {
            processor.property(i, key, &ColumnValue::String(v))?;
        } else if let Some(v) = value.float_value {
            processor.property(i, key, &ColumnValue::Float(v))?;
        } else if let Some(v) = value.double_value {
            processor.property(i, key, &ColumnValue::Double(v))?;
        } else if let Some(v) = value.int_value {
            processor.property(i, key, &ColumnValue::Long(v))?;
        } else if let Some(v) = value.uint_value {
            processor.property(i, key, &ColumnValue::ULong(v))?;
        } else if let Some(v) = value.sint_value {
            processor.property(i, key, &ColumnValue::Long(v))?;
        } else if let Some(v) = value.bool_value {
            processor.property(i, key, &ColumnValue::Bool(v))?;
        } else {
            return Err(MvtError::UnsupportedKeyValueType(key.clone()).into());
        }
    }
    processor.properties_end()
}

/// Filters `selected` (or `0..layer.features.len()`) down to features whose geometry decodes
/// cleanly, returning `(readable_indices, dropped_count)`.
pub(super) fn retain_readable_indices(
    layer: &tile::Layer,
    selected: Option<&[usize]>,
) -> (Vec<usize>, usize) {
    let total = selected.map_or(layer.features.len(), <[usize]>::len);
    let mut readable = Vec::with_capacity(total);
    match selected {
        Some(indices) => {
            for &idx in indices {
                if layer
                    .features
                    .get(idx)
                    .is_some_and(|feature| feature.process_geom(&mut ProcessorSink).is_ok())
                {
                    readable.push(idx);
                }
            }
        }
        None => {
            for (idx, feature) in layer.features.iter().enumerate() {
                if feature.process_geom(&mut ProcessorSink).is_ok() {
                    readable.push(idx);
                }
            }
        }
    }
    let dropped = total.saturating_sub(readable.len());
    (readable, dropped)
}

/// Lightweight metadata-only `tile::Layer` carrying `version`, `name`, and `extent` across worker
/// boundaries without retaining decoded feature/tag tables.
pub(super) fn layer_header(layer: &tile::Layer) -> tile::Layer {
    tile::Layer {
        version: layer.version,
        name: layer.name.clone(),
        extent: layer.extent,
        ..Default::default()
    }
}
