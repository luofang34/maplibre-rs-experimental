//! Query fixtures: a style whose test metadata carries `queryGeometry`, answered by
//! `queryRenderedFeatures` after the frame is drawn and compared with GL JS's `expected.json`.
//!
//! Features are compared in order by layer, source, source layer, id, properties and geometry
//! type; GL JS also reports each feature's geometry, which the renderer's query does not.

use std::path::Path;

use maplibre::{
    headless::map::HeadlessMap,
    query::QueriedFeature,
    sdf::query::{QueryGeometry, QueryOptions},
};
use serde_json::Value;

/// The query a fixture asks for.
pub(super) struct Query {
    geometry: QueryGeometry,
    options: QueryOptions,
}

/// The query in a fixture's test metadata, if it has one.
pub(super) fn of(test: &serde_json::Map<String, Value>) -> Result<Option<Query>, String> {
    let Some(geometry) = test.get("queryGeometry") else {
        return Ok(None);
    };
    let point = |value: &Value| -> Option<[f64; 2]> {
        let pair = value.as_array()?;
        Some([pair.first()?.as_f64()?, pair.get(1)?.as_f64()?])
    };
    let geometry = match (point(geometry), geometry.as_array()) {
        (Some(position), _) => QueryGeometry::Point(position),
        (None, Some(corners)) => match corners.as_slice() {
            [first, second] => QueryGeometry::Box {
                min: point(first).ok_or("Malformed query box corner")?,
                max: point(second).ok_or("Malformed query box corner")?,
            },
            _ => return Err(format!("Malformed queryGeometry: {geometry}")),
        },
        _ => return Err(format!("Malformed queryGeometry: {geometry}")),
    };
    let options = test.get("queryOptions");
    let layers = options
        .and_then(|options| options.get("layers"))
        .and_then(Value::as_array)
        .map(|layers| {
            layers
                .iter()
                .filter_map(|layer| layer.as_str().map(str::to_owned))
                .collect()
        });
    let filter = options.and_then(|options| options.get("filter")).cloned();
    Ok(Some(Query {
        geometry,
        options: QueryOptions { layers, filter },
    }))
}

/// The geometry type without GL JS's `Multi` prefix, which the renderer does not report.
fn base_type(geometry_type: &str) -> &str {
    geometry_type.strip_prefix("Multi").unwrap_or(geometry_type)
}

/// The fields a feature is compared by, from one of ours.
fn ours(feature: &QueriedFeature) -> Value {
    serde_json::json!({
        "layer": feature.layer,
        "source": feature.source,
        "sourceLayer": feature.source_layer,
        "id": feature.id,
        "properties": feature.properties,
        "type": base_type(feature.geometry_type),
    })
}

/// The same fields from a GL JS feature. A source layer GL JS leaves out, as it does for GeoJSON,
/// is taken as ours has it; a missing id means the feature has none.
fn theirs(expected: &Value, actual: &Value) -> Value {
    let field = |name: &str| {
        expected
            .get(name)
            .cloned()
            .unwrap_or_else(|| actual.get(name).cloned().unwrap_or(Value::Null))
    };
    serde_json::json!({
        "layer": expected
            .get("layer")
            .and_then(|layer| layer.get("id"))
            .cloned()
            .unwrap_or_else(|| actual["layer"].clone()),
        "source": field("source"),
        "sourceLayer": field("sourceLayer"),
        "id": expected.get("id").cloned().unwrap_or(Value::Null),
        "properties": expected.get("properties").cloned().unwrap_or(Value::Null),
        "type": expected
            .pointer("/geometry/type")
            .and_then(Value::as_str)
            .map(base_type)
            .map_or(Value::Null, |kind| Value::from(kind.to_owned())),
    })
}

/// Runs the query on the drawn map and returns whether it matches `expected.json`; writes the
/// compared fields of both to `actual.json`.
pub(super) fn compare(map: &HeadlessMap, query: &Query, test_dir: &Path) -> Result<bool, String> {
    let found = map
        .query_rendered_features(query.geometry, &query.options)
        .map_err(|error| format!("Query failed: {error}"))?;
    let expected: Vec<Value> = serde_json::from_str(
        &std::fs::read_to_string(test_dir.join("expected.json"))
            .map_err(|error| format!("Cannot read expected.json: {error}"))?,
    )
    .map_err(|error| format!("Cannot parse expected.json: {error}"))?;
    let actual: Vec<Value> = found.iter().map(ours).collect();
    let wanted: Vec<Value> = expected
        .iter()
        .enumerate()
        .map(|(index, feature)| theirs(feature, actual.get(index).unwrap_or(&Value::Null)))
        .collect();
    std::fs::write(
        test_dir.join("actual.json"),
        serde_json::to_string_pretty(&serde_json::json!({"actual": actual, "expected": wanted}))
            .map_err(|error| format!("Cannot write actual.json: {error}"))?,
    )
    .map_err(|error| format!("Cannot write actual.json: {error}"))?;
    Ok(actual == wanted)
}

#[cfg(test)]
mod tests;
