//! Hands each feature's geometry, id and geometry type to expressions that ask for them.

use serde_json::{json, Value};

use crate::style::{
    expression::{
        layer_reads_geometry, layer_reads_identity, GEOMETRY_PROPERTY, GEOMETRY_TYPE_PROPERTY,
        ID_PROPERTY,
    },
    filter::GeometryType,
    layer::StyleLayer,
};

/// Whether the layer reads anything the annotated GeoJSON adds to a feature's properties.
pub(super) fn reads_feature_inputs(layer: &StyleLayer) -> bool {
    layer_reads_geometry(layer) || layer_reads_identity(layer)
}

/// The GeoJSON with each feature's geometry, id and geometry type also written into its
/// properties, respecting `promoteId` and `generateId`.
pub(super) fn with_geometry_property(
    geojson: &Value,
    promoted: Option<&str>,
    generate_id: bool,
) -> Value {
    match geojson.get("type").and_then(Value::as_str) {
        Some("FeatureCollection") => {
            let mut collection = geojson.clone();
            if let Some(features) = collection.get_mut("features").and_then(Value::as_array_mut) {
                for (index, feature) in features.iter_mut().enumerate() {
                    annotate(feature, promoted, generate_id.then_some(index));
                }
            }
            collection
        }
        Some("Feature") => {
            let mut feature = geojson.clone();
            annotate(&mut feature, promoted, generate_id.then_some(0));
            feature
        }
        _ => {
            let mut feature = json!({"type": "Feature", "properties": {}, "geometry": geojson});
            annotate(&mut feature, promoted, generate_id.then_some(0));
            feature
        }
    }
}

fn annotate(feature: &mut Value, promoted: Option<&str>, generated: Option<usize>) {
    let Some(geometry) = feature.get("geometry") else {
        return;
    };
    let kind = geometry
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut added = vec![
        (GEOMETRY_PROPERTY, Value::String(geometry.to_string())),
        (
            GEOMETRY_TYPE_PROPERTY,
            Value::from(GeometryType::from_geojson(kind).name()),
        ),
    ];
    let resolved_id = if let Some(prop) = promoted {
        feature
            .get("properties")
            .and_then(|props| props.get(prop))
            .filter(|id| id.is_number() || id.is_string())
            .cloned()
    } else if let Some(index) = generated {
        Some(Value::from(index as u64))
    } else {
        feature
            .get("id")
            .filter(|id| id.is_number() || id.is_string())
            .cloned()
    };
    if let Some(id) = resolved_id {
        added.push((ID_PROPERTY, id));
    }
    if let Some(object) = feature.as_object_mut() {
        let properties = object.entry("properties").or_insert_with(|| json!({}));
        if !properties.is_object() {
            *properties = json!({});
        }
        if let Some(properties) = properties.as_object_mut() {
            for (key, value) in added {
                properties.insert(key.to_owned(), value);
            }
        }
    }
}
