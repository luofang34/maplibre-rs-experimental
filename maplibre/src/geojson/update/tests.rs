#![allow(clippy::expect_used, clippy::panic)]
use std::sync::Arc;

use serde_json::{json, Value};

use super::{GeoJsonDiff, SourceUpdateError};
use crate::style::{
    source::{GeoJsonData, Source},
    Style,
};

fn style(source: Value) -> Style {
    serde_json::from_value(json!({"version": 8,
        "sources": {"places": source, "roads": {"type": "vector", "tiles": ["t/{z}/{x}/{y}"]}},
        "layers": []}))
    .expect("style")
}

fn collection(features: Value) -> Value {
    json!({"type": "FeatureCollection", "features": features})
}

fn point(id: Value, name: &str, at: [f64; 2]) -> Value {
    json!({"type": "Feature", "id": id, "properties": {"name": name},
        "geometry": {"type": "Point", "coordinates": at}})
}

fn data(style: &Style) -> Value {
    let Some(Source::GeoJson(source)) = style.sources.get("places") else {
        panic!("geojson source");
    };
    let GeoJsonData::Inline(document) = &source.data else {
        panic!("inline data");
    };
    document.as_ref().clone()
}

fn generation(style: &Style) -> u64 {
    let Some(Source::GeoJson(source)) = style.sources.get("places") else {
        panic!("geojson source");
    };
    source.generation
}

fn names(style: &Style) -> Vec<String> {
    data(style)["features"]
        .as_array()
        .expect("features")
        .iter()
        .map(|feature| {
            feature["properties"]["name"]
                .as_str()
                .unwrap_or("")
                .to_owned()
        })
        .collect()
}

fn diff(value: Value) -> GeoJsonDiff {
    serde_json::from_value(value).expect("diff")
}

fn base() -> Style {
    style(json!({"type": "geojson", "data": collection(json!([
        point(json!(1), "one", [0.0, 0.0]),
        point(json!(2), "two", [1.0, 1.0])]))}))
}

#[test]
fn set_data_replaces_the_document_and_bumps_the_generation() {
    let mut style = base();
    assert_eq!(generation(&style), 0);
    let replacement = collection(json!([point(json!(9), "nine", [5.0, 5.0])]));
    style
        .set_geojson_data("places", GeoJsonData::Inline(Arc::new(replacement.clone())))
        .expect("set data");
    assert_eq!(data(&style), replacement);
    assert_eq!(generation(&style), 1);
    style
        .set_geojson_data(
            "places",
            GeoJsonData::Url("https://data.invalid/a.json".into()),
        )
        .expect("switch to a URL");
    assert_eq!(generation(&style), 2);
}

#[test]
fn invalid_data_is_rejected_and_the_source_keeps_what_it_had() {
    let mut style = base();
    let before = data(&style);
    let bad = collection(json!([{"type": "Feature",
        "geometry": {"type": "Point", "coordinates": "nowhere"}}]));
    let error = style
        .set_geojson_data("places", GeoJsonData::Inline(Arc::new(bad)))
        .expect_err("invalid geometry");
    assert!(matches!(error, SourceUpdateError::Invalid { .. }));
    assert_eq!((data(&style), generation(&style)), (before, 0));
}

#[test]
fn only_geojson_sources_can_be_updated() {
    let mut style = base();
    let empty = GeoJsonData::Inline(Arc::new(collection(json!([]))));
    assert!(matches!(
        style.set_geojson_data("missing", empty.clone()),
        Err(SourceUpdateError::UnknownSource { .. })
    ));
    assert!(matches!(
        style.set_geojson_data("roads", empty),
        Err(SourceUpdateError::NotGeoJson { .. })
    ));
}

#[test]
fn a_diff_adds_replaces_updates_and_removes_features_by_id() {
    let mut style = base();
    style
        .update_geojson_data(
            "places",
            &diff(json!({
                "remove": [1],
                "add": [point(json!(3), "three", [2.0, 2.0]), point(json!(2), "deux", [1.0, 1.0])],
                "update": [{"id": 3, "addOrUpdateProperties": [{"key": "size", "value": 4}],
                            "removeProperties": ["name"]}]
            })),
        )
        .expect("diff applies");
    let features = data(&style)["features"].clone();
    assert_eq!(features.as_array().expect("features").len(), 2);
    assert_eq!(
        features[0]["properties"]["name"], "deux",
        "same id is replaced in place"
    );
    assert_eq!(features[1]["properties"], json!({"size": 4}));
    assert_eq!(generation(&style), 1);
}

#[test]
fn a_diff_can_move_a_feature_and_clear_everything() {
    let mut style = base();
    style
        .update_geojson_data(
            "places",
            &diff(json!({"update": [{"id": 1,
                "newGeometry": {"type": "Point", "coordinates": [7.0, 7.0]},
                "removeAllProperties": true}]})),
        )
        .expect("move");
    let moved = data(&style)["features"][0].clone();
    assert_eq!(moved["geometry"]["coordinates"], json!([7.0, 7.0]));
    assert_eq!(moved["properties"], json!({}));
    style
        .update_geojson_data("places", &diff(json!({"removeAll": true})))
        .expect("clear");
    assert!(names(&style).is_empty());
}

#[test]
fn promoted_properties_identify_features_for_a_diff() {
    let mut style = style(json!({"type": "geojson", "promoteId": "code",
        "data": collection(json!([{"type": "Feature", "properties": {"code": "a", "name": "A"},
            "geometry": {"type": "Point", "coordinates": [0.0, 0.0]}}]))}));
    style
        .update_geojson_data(
            "places",
            &diff(json!({"update": [{"id": "a",
                "addOrUpdateProperties": [{"key": "name", "value": "Alpha"}]}]})),
        )
        .expect("update by promoted id");
    assert_eq!(names(&style), ["Alpha"]);
}

#[test]
fn invalid_diffs_change_nothing() {
    let mut style = base();
    let before = data(&style);
    for bad in [
        json!({"add": [{"type": "Feature", "properties": {},
            "geometry": {"type": "Point", "coordinates": [0, 0]}}]}),
        json!({"update": [{"id": 99}]}),
        json!({"remove": [1], "add": [{"type": "Feature", "id": 5,
            "geometry": {"type": "Point", "coordinates": false}}]}),
    ] {
        style
            .update_geojson_data("places", &diff(bad))
            .expect_err("rejected");
        assert_eq!((data(&style), generation(&style)), (before.clone(), 0));
    }
}

#[test]
fn feature_diffs_need_inline_data() {
    let mut style = style(json!({"type": "geojson", "data": "https://data.invalid/a.json"}));
    assert!(matches!(
        style.update_geojson_data("places", &diff(json!({"removeAll": true}))),
        Err(SourceUpdateError::NotInline { .. })
    ));
}
