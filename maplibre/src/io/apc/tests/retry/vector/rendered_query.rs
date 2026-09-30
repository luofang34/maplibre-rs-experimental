//! Fill and line features found through the geometry index the workers send.

use super::{
    super::fixture::{Fixture, Kind},
    tests::{geojson_style, WORLD_POLYGON},
};

fn screen_center(test: &Fixture) -> crate::sdf::query::QueryGeometry {
    let (width, height) = test.context.view_state.viewport_size();
    crate::sdf::query::QueryGeometry::Point([width / 2.0, height / 2.0])
}

#[tokio::test]
async fn a_rendered_polygon_is_found_under_the_screen_center_with_its_properties() {
    let mut test = Fixture::new(Kind::Vector, false).await;
    test.context.style = geojson_style(serde_json::json!({"type":"Feature","id":7,
        "properties":{"name":"world"},
        "geometry":{"type":"Polygon","coordinates":[[[-100,-60],[100,-60],[100,60],[-100,60],[-100,-60]]]}}));
    test.frame(0);
    test.receive().await;
    let found = test
        .context
        .query_rendered_features(screen_center(&test), &Default::default())
        .expect("query");
    assert_eq!(
        found.len(),
        1,
        "one polygon, however many tiles hold it: {found:?}"
    );
    assert_eq!(found[0].layer, "area");
    assert_eq!(found[0].source.as_deref(), Some("shapes"));
    assert_eq!(found[0].geometry_type, "Polygon");
    assert_eq!(found[0].id, Some(7));
    assert_eq!(found[0].properties["name"], "world");
}

#[tokio::test]
async fn a_layer_filter_and_removal_apply_to_the_live_style_at_query_time() {
    let mut test = Fixture::new(Kind::Vector, false).await;
    test.context.style = geojson_style(serde_json::from_str(WORLD_POLYGON).expect("polygon"));
    test.frame(0);
    test.receive().await;
    let rejecting = crate::sdf::query::QueryOptions {
        filter: Some(serde_json::json!(["==", "name", "nothing"])),
        ..Default::default()
    };
    assert!(test
        .context
        .query_rendered_features(screen_center(&test), &rejecting)
        .expect("query")
        .is_empty());
    assert_eq!(
        test.context
            .query_rendered_features(screen_center(&test), &Default::default())
            .expect("query")
            .len(),
        1
    );
    test.context.style.layers.clear();
    assert!(test
        .context
        .query_rendered_features(screen_center(&test), &Default::default())
        .expect("query")
        .is_empty());
}

#[tokio::test]
async fn a_polygon_away_from_the_point_is_not_found_and_a_nearby_line_is() {
    let mut test = Fixture::new(Kind::Vector, false).await;
    test.context.style = serde_json::from_value(serde_json::json!({"version":8,
        "sources":{"shapes":{"type":"geojson","data":{"type":"FeatureCollection","features":[
            {"type":"Feature","properties":{"k":"far"},"geometry":{"type":"Polygon",
                "coordinates":[[[100,40],[120,40],[120,50],[100,50],[100,40]]]}},
            {"type":"Feature","properties":{"k":"road"},"geometry":{"type":"LineString",
                "coordinates":[[-60,0.0],[60,0.0]]}}]}}},
        "layers":[
            {"id":"area","source":"shapes","type":"fill"},
            {"id":"road","source":"shapes","type":"line"}]}))
    .expect("style");
    test.frame(0);
    test.receive().await;
    let found = test
        .context
        .query_rendered_features(screen_center(&test), &Default::default())
        .expect("query");
    let layers: Vec<&str> = found.iter().map(|f| f.layer.as_str()).collect();
    assert_eq!(
        layers,
        ["road"],
        "the far polygon is not under the point: {found:?}"
    );
    assert_eq!(found[0].geometry_type, "LineString");
}

fn polygon_style(paint: serde_json::Value, filter: serde_json::Value) -> crate::style::Style {
    serde_json::from_value(serde_json::json!({"version":8,
        "sources":{"shapes":{"type":"geojson","data":{"type":"Feature","properties":{"k":1},
            "geometry":{"type":"Polygon","coordinates":[
                [[-60,-40],[60,-40],[60,40],[-60,40],[-60,-40]],
                [[-20,-10],[20,-10],[20,10],[-20,10],[-20,-10]]]}}}},
        "layers":[{"id":"outline","source":"shapes","type":"line","paint":paint,"filter":filter}]}))
    .expect("style")
}

async fn loaded(style: crate::style::Style) -> Fixture {
    let mut test = Fixture::new(Kind::Vector, false).await;
    test.context.style = style;
    test.frame(0);
    test.receive().await;
    test
}

#[tokio::test]
async fn a_line_layer_finds_polygon_rings_only_within_half_its_width() {
    // The hole edge is 10 degrees of latitude above the center, far outside 1px.
    let test = loaded(polygon_style(
        serde_json::json!({"line-width": 2}),
        serde_json::json!(["has", "k"]),
    ))
    .await;
    assert!(test
        .context
        .query_rendered_features(screen_center(&test), &Default::default())
        .expect("query")
        .is_empty());
    // At zoom 0 the hole's north edge is about 14px above the center.
    let (width, height) = test.context.view_state.viewport_size();
    let hole_edge = crate::sdf::query::QueryGeometry::Box {
        min: [width / 2.0, height / 2.0 - 30.0],
        max: [width / 2.0, height / 2.0],
    };
    let found = test
        .context
        .query_rendered_features(hole_edge, &Default::default())
        .expect("query");
    assert_eq!(found.len(), 1, "the rings are one feature: {found:?}");
    assert_eq!(found[0].geometry_type, "Polygon");
}

#[tokio::test]
async fn a_wide_line_reaches_further_than_a_thin_one() {
    let wide = loaded(polygon_style(
        serde_json::json!({"line-width": 400}),
        serde_json::json!(["has", "k"]),
    ))
    .await;
    assert_eq!(
        wide.context
            .query_rendered_features(screen_center(&wide), &Default::default())
            .expect("query")
            .len(),
        1,
        "the hole edge is inside half of 400px"
    );
}

#[tokio::test]
async fn a_layer_with_an_unparsable_filter_matches_nothing() {
    let test = loaded(polygon_style(
        serde_json::json!({"line-width": 400}),
        serde_json::json!(["no-such-operator", 1]),
    ))
    .await;
    assert!(test
        .context
        .query_rendered_features(screen_center(&test), &Default::default())
        .expect("query")
        .is_empty());
}
