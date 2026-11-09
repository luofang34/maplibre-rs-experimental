//! Hands each feature's geometry, id and geometry type to expressions that ask for them.

use serde_json::{json, Value};

use crate::style::{
    expression::{GEOMETRY_PROPERTY, GEOMETRY_TYPE_PROPERTY, ID_PROPERTY},
    filter::GeometryType,
    layer::StyleLayer,
};

/// Whether the layer's paint or layout reads the feature's id or geometry type.
pub(crate) fn reads_identity(layer: &StyleLayer) -> bool {
    serde_json::to_value(&layer.paint)
        .is_ok_and(|paint| crate::style::expression::reads_identity(&paint))
}

/// Whether the layer reads anything the annotated GeoJSON adds to a feature's properties.
pub(super) fn reads_feature_inputs(layer: &StyleLayer) -> bool {
    reads_geometry(layer) || reads_identity(layer)
}

/// Whether the layer has an expression that reads the feature's geometry.
pub(crate) fn reads_geometry(layer: &StyleLayer) -> bool {
    serde_json::to_string(layer)
        .is_ok_and(|text| text.contains("\"within\"") || text.contains("\"distance\""))
}

/// The GeoJSON with each feature's geometry, id and geometry type also written into its
/// properties.
pub(super) fn with_geometry_property(geojson: &Value) -> Value {
    match geojson.get("type").and_then(Value::as_str) {
        Some("FeatureCollection") => {
            let mut collection = geojson.clone();
            if let Some(features) = collection.get_mut("features").and_then(Value::as_array_mut) {
                features.iter_mut().for_each(annotate);
            }
            collection
        }
        Some("Feature") => {
            let mut feature = geojson.clone();
            annotate(&mut feature);
            feature
        }
        _ => {
            let mut feature = json!({"type": "Feature", "properties": {}, "geometry": geojson});
            annotate(&mut feature);
            feature
        }
    }
}

fn annotate(feature: &mut Value) {
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
    if let Some(id) = feature
        .get("id")
        .filter(|id| id.is_number() || id.is_string())
    {
        added.push((ID_PROPERTY, id.clone()));
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
