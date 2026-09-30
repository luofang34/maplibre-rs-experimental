#![allow(clippy::expect_used, clippy::panic)]
use geozero::mvt::{Message, Tile};
use serde_json::json;

use super::{GeoJsonError, GeoJsonIndex, EXTENT};
use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    style::source::{GeoJsonData, GeoJsonSource, PromoteId},
};

fn source(extra: serde_json::Value) -> GeoJsonSource {
    let mut declaration = json!({"data": {"type": "FeatureCollection", "features": []}});
    declaration
        .as_object_mut()
        .expect("object")
        .extend(extra.as_object().expect("object").clone());
    serde_json::from_value(declaration).expect("source")
}

fn tile(z: u8, x: i32, y: i32) -> WorldTileCoords {
    WorldTileCoords {
        x,
        y,
        z: ZoomLevel::from(z),
    }
}

fn index(document: serde_json::Value, extra: serde_json::Value) -> GeoJsonIndex {
    GeoJsonIndex::from_value(&document, &source(extra)).expect("document indexes")
}

fn features(bytes: &[u8]) -> Vec<geozero::mvt::tile::Feature> {
    let decoded = Tile::decode(bytes).expect("valid vector tile");
    assert_eq!(decoded.layers.len(), 1);
    assert_eq!(decoded.layers[0].name, "_geojson");
    assert_eq!(decoded.layers[0].extent, Some(EXTENT));
    decoded.layers[0].features.clone()
}

fn zigzag_decode(value: u32) -> i32 {
    ((value >> 1) as i32) ^ -((value & 1) as i32)
}

#[test]
fn a_point_lands_at_its_position_in_the_tile_grid_and_only_in_the_tiles_around_it() {
    let point = json!({"type": "Feature", "properties": {},
        "geometry": {"type": "Point", "coordinates": [10.0, -10.0]}});
    let index = index(point, json!({}));
    let hit = features(&index.tile(tile(1, 1, 1)));
    assert_eq!(hit.len(), 1);
    assert_eq!(hit[0].geometry[0], 9, "one MoveTo");
    let x = zigzag_decode(hit[0].geometry[1]);
    assert!(
        (x - 227).abs() <= 1,
        "longitude 10 sits 0.0556 tiles in: {x}"
    );
    assert!(
        features(&index.tile(tile(1, 0, 0))).is_empty(),
        "the north-west tile is more than a buffer away"
    );
}

#[test]
fn wrapped_worlds_repeat_the_same_tile() {
    let point = json!({"type": "Point", "coordinates": [179.9, 0.0]});
    let index = index(point, json!({}));
    assert_eq!(index.tile(tile(2, 3, 1)), index.tile(tile(2, -1, 1)));
    assert_eq!(features(&index.tile(tile(2, -1, 1))).len(), 1);
}

#[test]
fn properties_become_tags_shared_across_features() {
    let collection = json!({"type": "FeatureCollection", "features": [
        {"type": "Feature", "properties": {"name": "a", "rank": 2, "open": true, "gone": null},
         "geometry": {"type": "Point", "coordinates": [0.0, 0.0]}},
        {"type": "Feature", "properties": {"name": "a", "ratio": 0.5},
         "geometry": {"type": "Point", "coordinates": [0.001, 0.0]}}]});
    let index = index(collection, json!({}));
    let bytes = index.tile(tile(0, 0, 0));
    let layer = Tile::decode(bytes.as_slice())
        .expect("tile")
        .layers
        .remove(0);
    assert_eq!(layer.features.len(), 2);
    let mut keys = layer.keys.clone();
    keys.sort();
    assert_eq!(keys, ["name", "open", "rank", "ratio"], "null is dropped");
    assert_eq!(
        layer
            .values
            .iter()
            .filter(|value| value.string_value.as_deref() == Some("a"))
            .count(),
        1,
        "equal values are stored once"
    );
}

#[test]
fn identities_follow_id_promote_id_and_generate_id() {
    let collection = json!({"type": "FeatureCollection", "features": [
        {"type": "Feature", "id": 7, "properties": {"pid": 40},
         "geometry": {"type": "Point", "coordinates": [0.0, 0.0]}},
        {"type": "Feature", "properties": {"pid": 41},
         "geometry": {"type": "Point", "coordinates": [1.0, 0.0]}},
        {"type": "Feature", "properties": {},
         "geometry": {"type": "Point", "coordinates": [2.0, 0.0]}}]});
    let ids = |extra| {
        features(&index(collection.clone(), extra).tile(tile(0, 0, 0)))
            .iter()
            .map(|feature| feature.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(json!({})), [Some(7), None, None]);
    assert_eq!(
        ids(json!({"generateId": true})),
        [Some(7), Some(1), Some(2)]
    );
    assert_eq!(
        ids(json!({"promoteId": "pid"})),
        [Some(40), Some(41), None],
        "a promoted property replaces the feature id"
    );
    assert_eq!(
        ids(json!({"promoteId": {"_geojson": "pid"}, "generateId": true})),
        [Some(40), Some(41), Some(2)]
    );
    assert_eq!(
        source(json!({"promoteId": "pid"})).promote_id,
        Some(PromoteId::Property("pid".into()))
    );
}

#[test]
fn invalid_documents_report_what_is_wrong() {
    let declaration = source(json!({}));
    assert!(matches!(
        GeoJsonIndex::from_text(b"{not json", &declaration),
        Err(GeoJsonError::Json(_))
    ));
    assert!(matches!(
        GeoJsonIndex::from_value(&json!({"type": "Topology"}), &declaration),
        Err(GeoJsonError::Root { found }) if found == "Topology"
    ));
    let bad = json!({"type": "FeatureCollection", "features": [
        {"type": "Feature", "geometry": {"type": "Point", "coordinates": "here"}}]});
    assert!(matches!(
        GeoJsonIndex::from_value(&bad, &declaration),
        Err(GeoJsonError::Geometry { index: 0, .. })
    ));
}

#[test]
fn features_without_geometry_are_skipped() {
    let collection = json!({"type": "FeatureCollection", "features": [
        {"type": "Feature", "geometry": null, "properties": {}},
        {"type": "Feature", "geometry": {"type": "Point", "coordinates": [0, 0]}}]});
    assert_eq!(index(collection, json!({})).len(), 1);
}

#[test]
fn polygons_with_holes_and_reversed_winding_stay_fillable() {
    // Clockwise in longitude/latitude, the reverse of the specification's exterior ring.
    let polygon = json!({"type": "Polygon", "coordinates": [
        [[-10.0, -10.0], [-10.0, 10.0], [10.0, 10.0], [10.0, -10.0], [-10.0, -10.0]],
        [[-2.0, -2.0], [2.0, -2.0], [2.0, 2.0], [-2.0, 2.0], [-2.0, -2.0]]]});
    let index = index(polygon, json!({}));
    let feature = &features(&index.tile(tile(0, 0, 0)))[0];
    assert_eq!(
        feature.r#type,
        Some(geozero::mvt::tile::GeomType::Polygon as i32)
    );
    let (mut closes, mut cursor) = (0, 0);
    while cursor < feature.geometry.len() {
        let word = feature.geometry[cursor];
        let count = (word >> 3) as usize;
        match word & 7 {
            7 => {
                closes += 1;
                cursor += 1;
            }
            _ => cursor += 1 + 2 * count,
        }
    }
    assert_eq!(closes, 2, "the exterior ring and its hole are closed");
}

#[cfg(feature = "headless")]
#[test]
fn the_tile_tessellates_through_the_same_path_as_fetched_vector_tiles() {
    let polygon = json!({"type": "Polygon", "coordinates": [
        [[-10.0, -10.0], [10.0, -10.0], [10.0, 10.0], [-10.0, 10.0], [-10.0, -10.0]]]});
    let index = index(polygon, json!({}));
    let style: crate::style::Style = serde_json::from_value(json!({"version": 8,
        "sources": {}, "layers": [{"id": "area", "type": "fill", "source-layer": "_geojson",
            "paint": {"fill-color": "#ff0000"}}]}))
    .expect("style");
    let layers = crate::headless::map::process_tile_layers(
        &index.tile(tile(0, 0, 0)),
        &style.layers[0],
        tile(0, 0, 0),
        Default::default(),
    )
    .expect("tessellates");
    assert!(
        layers
            .vector
            .iter()
            .any(|layer| !layer.buffer.buffer.indices.is_empty()),
        "the polygon produces triangles"
    );
}

#[test]
fn inline_data_is_shared_not_copied_per_request() {
    let declaration = source(json!({"data": {"type": "Point", "coordinates": [0, 0]}}));
    let GeoJsonData::Inline(first) = &declaration.data else {
        panic!("inline data");
    };
    let clone = declaration.clone();
    let GeoJsonData::Inline(second) = &clone.data else {
        panic!("inline data");
    };
    assert!(std::sync::Arc::ptr_eq(first, second));
}

#[test]
fn generated_ids_count_the_position_in_the_document_even_past_skipped_features() {
    let collection = json!({"type": "FeatureCollection", "features": [
        {"type": "Feature", "geometry": null, "properties": {}},
        {"type": "Feature", "geometry": {"type": "Point", "coordinates": [0, 0]}}]});
    let ids: Vec<_> = features(&index(collection, json!({"generateId": true})).tile(tile(0, 0, 0)))
        .iter()
        .map(|feature| feature.id)
        .collect();
    assert_eq!(ids, [Some(1)]);
}

#[test]
fn clustering_sources_tile_their_points_as_clusters_until_the_cluster_zoom_passes() {
    let points = json!({"type": "FeatureCollection", "features": [
        {"type": "Feature", "properties": {"n": 1}, "geometry": {"type": "Point", "coordinates": [10.0, 10.0]}},
        {"type": "Feature", "properties": {"n": 2}, "geometry": {"type": "Point", "coordinates": [10.001, 10.0]}},
        {"type": "Feature", "properties": {"n": 3}, "geometry": {"type": "Point", "coordinates": [-100.0, -40.0]}},
    ]});
    let index = index(
        points,
        json!({"cluster": true, "clusterRadius": 40, "clusterMaxZoom": 3}),
    );
    let low = index.tile(tile(0, 0, 0));
    assert_eq!(
        features(&low).len(),
        2,
        "the two close points make one cluster"
    );
    let decoded = Tile::decode(low.as_slice()).expect("tile");
    assert!(decoded.layers[0].keys.contains(&"point_count".to_owned()));
    let deep = features(&index.tile(tile(6, 34, 25)));
    assert!(
        deep.len() <= 1,
        "past the cluster zoom every point stands alone"
    );
}
