#![allow(clippy::expect_used, clippy::panic)]
use std::sync::Arc;

use serde_json::{json, Value};

use super::{SourceFeature, SourceQueryError, SourceQueryOptions};
use crate::style::{source::GeoJsonData, Style};

fn style(source: Value) -> Style {
    serde_json::from_value(json!({"version": 8,
        "sources": {"places": source, "roads": {"type": "vector", "tiles": ["t/{z}/{x}/{y}"]}},
        "layers": []}))
    .expect("style")
}

fn feature(id: u64, kind: &str, rank: u64, geometry: Value) -> Value {
    json!({"type": "Feature", "id": id, "properties": {"kind": kind, "rank": rank},
        "geometry": geometry})
}

fn point(at: [f64; 2]) -> Value {
    json!({"type": "Point", "coordinates": at})
}

fn places() -> Style {
    style(
        json!({"type": "geojson", "data": {"type": "FeatureCollection", "features": [
        feature(1, "town", 2, point([0.0, 0.0])),
        feature(2, "city", 9, json!({"type": "LineString", "coordinates": [[0, 0], [1, 1]]})),
        {"type": "Feature", "properties": {}, "geometry": null},
        feature(4, "town", 5, point([2.0, 2.0]))]}}),
    )
}

fn ids(found: &[SourceFeature]) -> Vec<Option<u64>> {
    found.iter().map(|feature| feature.id).collect()
}

#[test]
fn every_feature_comes_back_in_document_order_with_its_attributes() {
    let found = places()
        .query_source_features("places", &SourceQueryOptions::default())
        .expect("query");
    assert_eq!(ids(&found), [Some(1), Some(2), Some(4)]);
    assert_eq!(found[0].source, "places");
    assert_eq!(found[0].source_layer, "_geojson");
    assert_eq!(found[0].geometry_type, "Point");
    assert_eq!(found[0].properties["kind"], "town");
    assert_eq!(found[1].geometry_type, "LineString");
    assert_eq!(found[1].geometry["coordinates"], json!([[0, 0], [1, 1]]));
}

#[test]
fn filters_use_the_legacy_and_the_expression_syntax_and_geometry_type() {
    let query = |filter: Value| {
        ids(&places()
            .query_source_features(
                "places",
                &SourceQueryOptions {
                    source_layer: None,
                    filter: Some(filter),
                },
            )
            .expect("query"))
    };
    assert_eq!(query(json!(["==", "kind", "town"])), [Some(1), Some(4)]);
    assert_eq!(query(json!([">", ["get", "rank"], 4])), [Some(2), Some(4)]);
    assert_eq!(query(json!(["==", "$type", "LineString"])), [Some(2)]);
    assert_eq!(query(json!(["==", "$id", 4])), [Some(4)]);
    assert!(query(json!(["==", "kind", "village"])).is_empty());
}

#[test]
fn only_the_geojson_layer_can_be_named_as_the_source_layer() {
    let options = |layer: &str| SourceQueryOptions {
        source_layer: Some(layer.into()),
        filter: None,
    };
    assert_eq!(
        places()
            .query_source_features("places", &options("_geojson"))
            .expect("query")
            .len(),
        3
    );
    assert!(places()
        .query_source_features("places", &options("water"))
        .expect("query")
        .is_empty());
}

#[test]
fn ids_follow_promote_id_and_generate_id() {
    let style = style(
        json!({"type": "geojson", "promoteId": "code", "generateId": true,
        "data": {"type": "FeatureCollection", "features": [
            {"type": "Feature", "properties": {"code": "a"}, "geometry": {"type": "Point", "coordinates": [0, 0]}},
            {"type": "Feature", "properties": {}, "geometry": {"type": "Point", "coordinates": [1, 1]}}]}}),
    );
    let found = style
        .query_source_features("places", &SourceQueryOptions::default())
        .expect("query");
    assert_eq!(
        ids(&found),
        [Some(0), Some(1)],
        "a string id is not carried into tiles, so a source query reports the generated one"
    );
}

#[test]
fn the_answer_follows_set_data_and_update_data() {
    let mut style = places();
    style
        .set_geojson_data(
            "places",
            GeoJsonData::Inline(Arc::new(json!({"type": "Point", "coordinates": [3, 3]}))),
        )
        .expect("set data");
    let found = style
        .query_source_features("places", &SourceQueryOptions::default())
        .expect("query");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].geometry["coordinates"], json!([3, 3]));
}

#[test]
fn unavailable_sources_are_typed_errors() {
    let style = style(json!({"type": "geojson", "data": "https://data.invalid/a.json"}));
    let options = SourceQueryOptions::default();
    assert_eq!(
        style.query_source_features("places", &options),
        Err(SourceQueryError::NotLoaded {
            source_name: "places".into()
        })
    );
    assert!(matches!(
        style.query_source_features("roads", &options),
        Err(SourceQueryError::NotGeoJson { .. })
    ));
    assert!(matches!(
        style.query_source_features("nowhere", &options),
        Err(SourceQueryError::UnknownSource { .. })
    ));
    assert!(matches!(
        places().query_source_features(
            "places",
            &SourceQueryOptions {
                source_layer: None,
                filter: Some(json!(7))
            }
        ),
        Err(SourceQueryError::InvalidFilter(_))
    ));
}
