#![allow(clippy::expect_used, clippy::panic)]
use std::sync::Arc;

use serde_json::{json, Value};

use super::{SourceFeature, SourceQueryError, SourceQueryOptions};
use crate::style::{feature_state::QueriedFeatureId, source::GeoJsonData, Style};

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

fn ids(found: &[SourceFeature]) -> Vec<Option<QueriedFeatureId>> {
    found.iter().map(|feature| feature.id.clone()).collect()
}

fn num(n: u64) -> Option<QueriedFeatureId> {
    Some(QueriedFeatureId::Number(n))
}

#[test]
fn every_feature_comes_back_in_document_order_with_its_attributes() {
    let found = places()
        .query_source_features("places", &SourceQueryOptions::default())
        .expect("query");
    assert_eq!(ids(&found), [num(1), num(2), num(4)]);
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
    assert_eq!(query(json!(["==", "kind", "town"])), [num(1), num(4)]);
    assert_eq!(query(json!([">", ["get", "rank"], 4])), [num(2), num(4)]);
    assert_eq!(query(json!(["==", "$type", "LineString"])), [num(2)]);
    assert_eq!(query(json!(["==", "$id", 4])), [num(4)]);
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
    let style_promoted = style(
        json!({"type": "geojson", "promoteId": "code", "generateId": true,
        "data": {"type": "FeatureCollection", "features": [
            {"type": "Feature", "id": 99, "properties": {"code": "a"}, "geometry": {"type": "Point", "coordinates": [0, 0]}},
            {"type": "Feature", "id": 100, "properties": {}, "geometry": {"type": "Point", "coordinates": [1, 1]}}]}}),
    );
    let found = style_promoted
        .query_source_features("places", &SourceQueryOptions::default())
        .expect("query");
    assert_eq!(
        ids(&found),
        [Some(QueriedFeatureId::String("a".into())), None],
        "promoteId preserves string IDs and does not fall back to feature.id or generateId when missing"
    );

    let style_gen = style(json!({"type": "geojson", "generateId": true,
        "data": {"type": "FeatureCollection", "features": [
            {"type": "Feature", "id": 99, "properties": {}, "geometry": {"type": "Point", "coordinates": [0, 0]}},
            {"type": "Feature", "properties": {}, "geometry": {"type": "Point", "coordinates": [1, 1]}}]}}));
    let gen_found = style_gen
        .query_source_features("places", &SourceQueryOptions::default())
        .expect("query");
    assert_eq!(
        ids(&gen_found),
        [num(0), num(1)],
        "generateId overwrites existing feature.id per spec"
    );
}

#[test]
fn source_filter_is_applied_and_invalid_source_filter_returns_empty() {
    let filtered = style(json!({
        "type": "geojson",
        "filter": ["==", ["get", "kind"], "town"],
        "data": {"type": "FeatureCollection", "features": [
            feature(1, "town", 2, point([0.0, 0.0])),
            feature(2, "city", 9, point([1.0, 1.0])),
            feature(4, "town", 5, point([2.0, 2.0]))
        ]}
    }));
    let found = filtered
        .query_source_features("places", &SourceQueryOptions::default())
        .expect("query");
    assert_eq!(ids(&found), [num(1), num(4)]);

    let invalid = style(json!({
        "type": "geojson",
        "filter": 7,
        "data": {"type": "FeatureCollection", "features": [
            feature(1, "town", 2, point([0.0, 0.0]))
        ]}
    }));
    let empty = invalid
        .query_source_features("places", &SourceQueryOptions::default())
        .expect("invalid source filter handled cleanly");
    assert!(empty.is_empty());
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
