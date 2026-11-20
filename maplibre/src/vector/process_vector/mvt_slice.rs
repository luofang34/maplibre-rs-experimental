//! Zero-clone MVT layer iteration, feature-index filtering, and header construction.

use std::sync::Arc;

use geozero::{
    error::Result as GeoResult,
    mvt::{process_geom, tile, Message, MvtError},
    ColumnValue, FeatureProcessor, GeozeroGeometry, ProcessorSink,
};

mod props;
pub(crate) use props::fill_feature_properties;
pub(super) use props::{decode_layer_properties, decode_layer_properties_bounded};

use crate::{
    style::{
        expression::{FeatureProperties, Value},
        filter::{FeatureContext, Filter, GeometryType},
    },
    vector::tessellation::feature_paint::FeatureStateSyncContext,
};

/// Evaluates `filter` against `layer.features` without cloning or mutating `layer`, reusing
/// `shared_props` when available (or one scratch map otherwise) and returning matching feature
/// indices in layer order.
pub(super) fn select_filtered_features(
    layer: &tile::Layer,
    filter: &Filter,
    zoom: f64,
    state_ctx: Option<&FeatureStateSyncContext>,
    shared_props: Option<&[Arc<FeatureProperties>]>,
) -> Vec<usize> {
    let mut selected = Vec::new();
    let mut scratch = FeatureProperties::default();
    for (index, feature) in layer.features.iter().enumerate() {
        let props_ref: &FeatureProperties =
            if let Some(shared) = shared_props.and_then(|all| all.get(index)) {
                shared.as_ref()
            } else {
                fill_feature_properties(layer, feature, &mut scratch);
                if let Some(ctx) = state_ctx {
                    crate::style::feature_state::sync_properties_feature_state(
                        &mut scratch,
                        feature.id,
                        ctx.promote_property.as_deref(),
                        ctx.source_state.as_deref(),
                        ctx.source_layer.as_deref(),
                    );
                }
                &scratch
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

pub(super) fn process_feature_properties(
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

/// Decodes raw MVT bytes on demand into tile-local [`crate::io::geometry_index::IndexedGeometry`]
/// entries for point and box feature queries.
#[cfg(test)]
pub(crate) fn index_mvt_bytes(
    mvt_bytes: &[u8],
    promote_id: Option<&crate::style::source::PromoteId>,
    feature_state: Option<&Arc<crate::style::feature_state::SourceFeatureState>>,
) -> Vec<crate::io::geometry_index::IndexedGeometry<f64>> {
    super::decode::index_mvt_bytes_rescaled(mvt_bytes, None, promote_id, feature_state)
}

pub(super) fn index_decoded_mvt_tile(
    tile: &geozero::mvt::Tile,
    promote_id: Option<&crate::style::source::PromoteId>,
    feature_state: Option<&Arc<crate::style::feature_state::SourceFeatureState>>,
) -> Vec<crate::io::geometry_index::IndexedGeometry<f64>> {
    let mut index = crate::io::geometry_index::IndexProcessor::new();
    for layer in &tile.layers {
        index.set_coordinate_scale(super::extent_scale(layer));
        let state_ctx =
            (promote_id.is_some() || feature_state.is_some()).then(|| FeatureStateSyncContext {
                promote_property: promote_id
                    .and_then(|p| p.property_for(Some(&layer.name)))
                    .map(str::to_owned),
                source_state: feature_state.cloned(),
                source_layer: Some(layer.name.clone()),
            });
        let shared = decode_layer_properties(layer, state_ctx.as_ref());
        index.set_feature_state_context(state_ctx);
        index.begin_layer(
            &layer.name,
            layer.features.iter().map(|feature| feature.id).collect(),
        );
        let skip_props = shared.is_some();
        if let Some(props) = shared {
            index.set_shared_properties(props);
        }
        let _ = process_mvt_slice(layer, None, skip_props, &mut index);
    }
    index.get_geometries()
}

/// Non-allocating check returning `true` when `data` is an empty/zero-padded buffer or frames
/// cleanly as top-level MVT `Tile.layers` records (`field 3, wire_type 2`) with valid layer names.
#[cfg(feature = "mlt")]
pub(crate) fn looks_like_mvt(data: &[u8]) -> bool {
    let end = data
        .iter()
        .rposition(|byte| *byte != 0)
        .map_or(0, |idx| idx + 1);
    let bytes = &data[..end];
    if bytes.is_empty() {
        return true;
    }
    let mut pos = 0;
    let mut saw_mvt_layer = false;
    while pos < bytes.len() {
        let Some(tag) = read_varint(bytes, &mut pos) else {
            return false;
        };
        let field_num = tag >> 3;
        let wire_type = tag & 0x07;
        if field_num == 3 && wire_type == 2 {
            let Some(len) = read_varint(bytes, &mut pos).and_then(|v| usize::try_from(v).ok())
            else {
                return false;
            };
            let Some(slice_end) = pos.checked_add(len) else {
                return false;
            };
            let Some(layer_bytes) = bytes.get(pos..slice_end) else {
                return false;
            };
            pos = slice_end;
            if scan_mvt_layer_name(layer_bytes).is_none() {
                return false;
            }
            saw_mvt_layer = true;
        } else if field_num >= 16 {
            if skip_wire_value(bytes, &mut pos, wire_type).is_none() {
                return false;
            }
        } else {
            return false;
        }
    }
    saw_mvt_layer
}

mod filtered;
pub(super) use filtered::try_decode_mvt_filtered_with_keys_rescaled;

/// Scans raw MVT protobuf wire tags (`Tile.layers = 3`, `Layer.name = 1`) and decodes only layers
/// matching `keep`, avoiding `prost` allocations for unused source layers. Returns `None` when
/// `data` is not a multi-layer or non-empty MVT wire stream so `decode_tile` can handle empty,
/// malformed, or MapLibre Tile (`MLT`) payloads identically.
pub(super) fn try_decode_mvt_filtered(
    data: &[u8],
    keep: impl Fn(&str) -> bool,
) -> Option<geozero::mvt::Tile> {
    let end = data
        .iter()
        .rposition(|byte| *byte != 0)
        .map_or(0, |idx| idx + 1);
    let bytes = &data[..end];
    if bytes.is_empty() {
        return None;
    }
    let mut pos = 0;
    let mut saw_mvt_layer = false;
    let mut layers = Vec::new();
    while pos < bytes.len() {
        let tag = read_varint(bytes, &mut pos)?;
        let field_num = tag >> 3;
        let wire_type = tag & 0x07;
        if field_num == 3 && wire_type == 2 {
            let len = usize::try_from(read_varint(bytes, &mut pos)?).ok()?;
            let slice_end = pos.checked_add(len)?;
            let layer_bytes = bytes.get(pos..slice_end)?;
            pos = slice_end;
            let name = scan_mvt_layer_name(layer_bytes)?;
            saw_mvt_layer = true;
            if keep(name) {
                let layer = tile::Layer::decode(layer_bytes).ok()?;
                layers.push(layer);
            }
        } else if field_num >= 16 {
            skip_wire_value(bytes, &mut pos, wire_type)?;
        } else {
            return None;
        }
    }
    saw_mvt_layer.then_some(geozero::mvt::Tile { layers })
}

fn scan_mvt_layer_name(layer_bytes: &[u8]) -> Option<&str> {
    let mut pos = 0;
    while pos < layer_bytes.len() {
        let tag = read_varint(layer_bytes, &mut pos)?;
        let field_num = tag >> 3;
        let wire_type = tag & 0x07;
        if field_num == 0 {
            return None;
        }
        if field_num == 1 && wire_type == 2 {
            let len = usize::try_from(read_varint(layer_bytes, &mut pos)?).ok()?;
            let end = pos.checked_add(len)?;
            let name_bytes = layer_bytes.get(pos..end)?;
            return std::str::from_utf8(name_bytes).ok();
        }
        skip_wire_value(layer_bytes, &mut pos, wire_type)?;
    }
    None
}

fn read_varint(bytes: &[u8], pos: &mut usize) -> Option<u64> {
    let mut value = 0_u64;
    let mut shift = 0_u32;
    while shift < 64 {
        let &byte = bytes.get(*pos)?;
        *pos += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
        shift += 7;
    }
    None
}

fn skip_wire_value(bytes: &[u8], pos: &mut usize, wire_type: u64) -> Option<()> {
    match wire_type {
        0 => {
            read_varint(bytes, pos)?;
            Some(())
        }
        1 => {
            *pos = pos.checked_add(8)?;
            (*pos <= bytes.len()).then_some(())
        }
        2 => {
            let len = usize::try_from(read_varint(bytes, pos)?).ok()?;
            *pos = pos.checked_add(len)?;
            (*pos <= bytes.len()).then_some(())
        }
        5 => {
            *pos = pos.checked_add(4)?;
            (*pos <= bytes.len()).then_some(())
        }
        _ => None,
    }
}
