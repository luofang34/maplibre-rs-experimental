//! Hands each feature's geometry to expressions that ask for it.

use serde_json::{json, Value};

use crate::style::{expression::GEOMETRY_PROPERTY, layer::StyleLayer};

/// Whether the layer has an expression that reads the feature's geometry.
pub(super) fn reads_geometry(layer: &StyleLayer) -> bool {
    serde_json::to_string(layer)
        .is_ok_and(|text| text.contains("\"within\"") || text.contains("\"distance\""))
}

/// The GeoJSON with each feature's geometry also written into its properties.
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
            json!({"type": "Feature", "properties": {GEOMETRY_PROPERTY: geojson.to_string()}, "geometry": geojson})
        }
    }
}

fn annotate(feature: &mut Value) {
    let Some(geometry) = feature.get("geometry").map(Value::to_string) else {
        return;
    };
    if let Some(object) = feature.as_object_mut() {
        let properties = object.entry("properties").or_insert_with(|| json!({}));
        if let Some(properties) = properties.as_object_mut() {
            properties.insert(GEOMETRY_PROPERTY.to_owned(), Value::String(geometry));
        }
    }
}
