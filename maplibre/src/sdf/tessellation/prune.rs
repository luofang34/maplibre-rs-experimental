//! Runtime symbol property pruning and source-feature index tracking.

use std::sync::Arc;

use rustc_hash::FxHashMap;

use super::{layout::SourceFeature, TextTessellator};
use crate::{
    sdf::Feature,
    style::{
        expression::{
            empty_feature_properties, symbol_runtime_referenced_keys, FeatureProperties,
            FEATURE_STATE_PREFIX,
        },
        filter::GeometryType,
        layer::StyleLayer,
    },
};

/// Maps the local geometry slice index to its original MVT/MLT feature index in the source layer.
pub(crate) fn source_feature_indices(count: usize, slice: Option<&[usize]>) -> Vec<Option<u32>> {
    match slice {
        Some(indices) => indices.iter().map(|&idx| u32::try_from(idx).ok()).collect(),
        None => (0..count).map(|idx| u32::try_from(idx).ok()).collect(),
    }
}

pub(super) fn build_source_feature(
    tessellator: &mut TextTessellator,
    slice_idx: Option<usize>,
    geometry: &geo_types::Geometry<f64>,
) -> (SourceFeature, Arc<FeatureProperties>) {
    let id = slice_idx
        .and_then(|i| tessellator.source_ids.get(i).copied())
        .flatten();
    let feature_index = slice_idx
        .and_then(|i| tessellator.source_indices.get(i).copied())
        .flatten();
    let props = tessellator.take_feature_properties(slice_idx, id);
    let kind = match geometry {
        geo_types::Geometry::Point(_) | geo_types::Geometry::MultiPoint(_) => GeometryType::Point,
        geo_types::Geometry::LineString(_) | geo_types::Geometry::MultiLineString(_) => {
            GeometryType::LineString
        }
        geo_types::Geometry::Polygon(_) | geo_types::Geometry::MultiPolygon(_) => {
            GeometryType::Polygon
        }
        _ => GeometryType::Unknown,
    };
    (
        SourceFeature {
            id,
            kind,
            feature_index,
        },
        props,
    )
}

/// Drops feature property entries that are only read during tessellation (`text-field`,
/// `icon-image`, `filter`, layout-only keys) once symbol layout finishes, retaining only runtime
/// placement/paint properties, `\u{1}*` internal state/identity tags, and `promote_id_key`.
pub(crate) fn prune_symbol_features(
    features: &mut [Feature],
    style: &StyleLayer,
    promote_id_key: Option<&str>,
) {
    let Some(needed) = symbol_runtime_referenced_keys(style, promote_id_key) else {
        return;
    };
    let empty = empty_feature_properties();
    let mut cache: FxHashMap<usize, Arc<FeatureProperties>> = FxHashMap::default();

    for feature in features {
        if feature.data.properties.is_empty() {
            continue;
        }
        let needs_prune = feature.data.properties.keys().any(|k| {
            !k.starts_with('\u{1}') && !k.starts_with(FEATURE_STATE_PREFIX) && !needed.contains(k)
        });
        if !needs_prune {
            continue;
        }
        let ptr = Arc::as_ptr(&feature.data.properties) as usize;
        if let Some(pruned) = cache.get(&ptr) {
            feature.data.properties = Arc::clone(pruned);
            continue;
        }
        let mut filtered = FeatureProperties::default();
        for (k, v) in feature.data.properties.iter() {
            if k.starts_with('\u{1}') || k.starts_with(FEATURE_STATE_PREFIX) || needed.contains(k) {
                filtered.insert(k.clone(), v.clone());
            }
        }
        let pruned = if filtered.is_empty() {
            Arc::clone(empty)
        } else {
            Arc::new(filtered)
        };
        cache.insert(ptr, Arc::clone(&pruned));
        feature.data.properties = pruned;
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{
        sdf::SymbolFeatureData,
        style::{
            expression::Value,
            layer::{LayerPaint, SymbolPaint},
        },
    };

    #[test]
    fn prune_symbol_features_drops_tessellation_only_keys_and_shares_empty_arc() {
        let mut props_json = serde_json::Map::new();
        props_json.insert("text-color".to_owned(), json!(["get", "tint"]));
        let style = StyleLayer {
            id: "places".to_owned(),
            type_: "symbol".to_owned(),
            paint: Some(LayerPaint::Symbol(SymbolPaint {
                text_field: Some(crate::style::layer::StyleProperty::parse(&json!("{name}"))),
                properties: props_json,
                ..Default::default()
            })),
            ..Default::default()
        };

        let mut f_only_name = FeatureProperties::default();
        f_only_name.insert("name".into(), Value::String("Zürich".into()));
        f_only_name.insert("name:fr".into(), Value::String("Zurich".into()));

        let mut f_with_runtime = FeatureProperties::default();
        f_with_runtime.insert("name".into(), Value::String("Bern".into()));
        f_with_runtime.insert("tint".into(), Value::String("#112233".into()));
        f_with_runtime.insert(format!("{FEATURE_STATE_PREFIX}hover"), Value::Bool(true));

        let mut features = vec![
            Feature {
                parts: [None, None, None],
                data: SymbolFeatureData {
                    properties: Arc::new(f_only_name),
                    source_feature_index: Some(0),
                    ..Default::default()
                },
                bbox: crate::euclid::Box2D::zero(),
                indices: 0..0,
                text_anchor: crate::euclid::Point2D::new(0.0, 0.0),
                anchor_shifts: Vec::new(),
                text_sets: Vec::new(),
                anchor_sets: Vec::new(),
                text_colors: Vec::new(),
                fallback: false,
                repeat_key: None,
                str: String::new(),
                line: None,
            },
            Feature {
                parts: [None, None, None],
                data: SymbolFeatureData {
                    properties: Arc::new(f_with_runtime),
                    source_feature_index: Some(1),
                    ..Default::default()
                },
                bbox: crate::euclid::Box2D::zero(),
                indices: 0..0,
                text_anchor: crate::euclid::Point2D::new(0.0, 0.0),
                anchor_shifts: Vec::new(),
                text_sets: Vec::new(),
                anchor_sets: Vec::new(),
                text_colors: Vec::new(),
                fallback: false,
                repeat_key: None,
                str: String::new(),
                line: None,
            },
        ];

        prune_symbol_features(&mut features, &style, None);
        assert!(features[0].data.properties.is_empty());
        assert!(Arc::ptr_eq(
            &features[0].data.properties,
            empty_feature_properties()
        ));
        assert_eq!(features[1].data.properties.len(), 2);
        assert_eq!(
            features[1].data.properties.get("tint"),
            Some(&Value::String("#112233".into()))
        );
        assert!(features[1]
            .data
            .properties
            .contains_key(&format!("{FEATURE_STATE_PREFIX}hover")));
        assert!(!features[1].data.properties.contains_key("name"));
    }
}
