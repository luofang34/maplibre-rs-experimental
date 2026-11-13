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

/// Serializes GeoJSON for `geozero`'s streaming reader with `"properties"` emitted before
/// `"geometry"` (and standalone `"Feature"` values wrapped in a one-element `"FeatureCollection"`),
/// so geometry tessellators that inspect `feature_properties` (`fill-extrusion-*`, `line-width`,
/// `line-elevation`, etc.) always see the feature's attributes during geometry callbacks.
pub(super) fn streamable_geojson(geojson: &Value) -> String {
    match geojson.get("type").and_then(Value::as_str) {
        Some("FeatureCollection") => {
            let Some(features) = geojson.get("features").and_then(Value::as_array) else {
                return geojson.to_string();
            };
            let body: Vec<String> = features.iter().map(serialize_feature_props_first).collect();
            format!(
                "{{\"type\":\"FeatureCollection\",\"features\":[{}]}}",
                body.join(",")
            )
        }
        Some("Feature") => format!(
            "{{\"type\":\"FeatureCollection\",\"features\":[{}]}}",
            serialize_feature_props_first(geojson)
        ),
        _ => geojson.to_string(),
    }
}

fn serialize_feature_props_first(feature: &Value) -> String {
    let Some(obj) = feature.as_object() else {
        return feature.to_string();
    };
    let mut parts = vec![String::from("\"type\":\"Feature\"")];
    if let Some(id) = obj.get("id") {
        parts.push(format!("\"id\":{id}"));
    }
    let props = obj
        .get("properties")
        .filter(|v| v.is_object())
        .map_or_else(|| String::from("{}"), Value::to_string);
    parts.push(format!("\"properties\":{props}"));
    let geom = obj
        .get("geometry")
        .map_or_else(|| String::from("null"), Value::to_string);
    parts.push(format!("\"geometry\":{geom}"));
    format!("{{{}}}", parts.join(","))
}
