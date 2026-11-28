//! Indexed GeoJSON source caching, clustering, filtering, and spatial tiling.

use crate::style::{
    expression::Value,
    filter::{properties_from_json, FeatureContext, Filter, GeometryType},
};

pub(crate) mod cluster;
pub mod index;
pub(crate) mod store;

/// Whether one GeoJSON feature passes a layer filter.
pub(crate) fn feature_passes(feature: &serde_json::Value, filter: &Filter, zoom: f64) -> bool {
    let properties = properties_from_json(feature.get("properties"));
    let geometry_type = feature
        .get("geometry")
        .and_then(|geometry| geometry.get("type"))
        .and_then(serde_json::Value::as_str)
        .map_or(GeometryType::Unknown, GeometryType::from_geojson);
    filter.evaluate(&FeatureContext {
        properties: &properties,
        geometry_type,
        id: feature.get("id").map(Value::from_json),
        zoom,
    })
}

/// The GeoJSON a layer sees after its filter: a collection keeps the passing features, a lone
/// feature or geometry either stays or becomes an empty collection.
pub fn filter_geojson(
    geojson: &serde_json::Value,
    filter: &Filter,
    zoom: f64,
) -> serde_json::Value {
    let empty = serde_json::json!({"type": "FeatureCollection", "features": []});
    match geojson.get("type").and_then(serde_json::Value::as_str) {
        Some("FeatureCollection") => {
            let features: Vec<serde_json::Value> = geojson
                .get("features")
                .and_then(serde_json::Value::as_array)
                .map(|features| {
                    features
                        .iter()
                        .filter(|feature| feature_passes(feature, filter, zoom))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            serde_json::json!({"type": "FeatureCollection", "features": features})
        }
        Some("Feature") if feature_passes(geojson, filter, zoom) => geojson.clone(),
        Some("Feature") => empty,
        Some(_) => {
            let feature = serde_json::json!({"type": "Feature", "geometry": geojson});
            if feature_passes(&feature, filter, zoom) {
                geojson.clone()
            } else {
                empty
            }
        }
        None => geojson.clone(),
    }
}
