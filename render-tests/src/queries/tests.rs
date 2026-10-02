use serde_json::json;

use super::{of, ours, theirs};
use maplibre::{query::QueriedFeature, sdf::query::QueryGeometry};

fn test(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    value.as_object().expect("object").clone()
}

#[test]
fn a_point_and_a_box_are_read_with_their_options() {
    let point = of(&test(json!({"queryGeometry": [10, 20]})))
        .expect("parses")
        .expect("a query");
    assert!(matches!(point.geometry, QueryGeometry::Point([x, y]) if x == 10.0 && y == 20.0));
    let boxed = of(&test(json!({
        "queryGeometry": [[1, 2], [3, 4]],
        "queryOptions": {"layers": ["a"], "filter": ["==", "x", 1]}
    })))
    .expect("parses")
    .expect("a query");
    assert!(matches!(
        boxed.geometry,
        QueryGeometry::Box {
            min: [1.0, 2.0],
            max: [3.0, 4.0]
        }
    ));
    assert_eq!(boxed.options.layers, Some(vec!["a".to_owned()]));
    assert_eq!(boxed.options.filter, Some(json!(["==", "x", 1])));
    assert!(of(&test(json!({}))).expect("parses").is_none());
    assert!(of(&test(json!({"queryGeometry": [[1, 2]]}))).is_err());
}

#[test]
fn features_compare_by_their_fields_without_geometry() {
    let feature = QueriedFeature {
        layer: "zones".into(),
        source: Some("zones".into()),
        source_layer: "_geojsonTileLayer".into(),
        id: None,
        properties: [("name".to_owned(), json!("a"))].into(),
        geometry_type: "Polygon",
        text: None,
    };
    let actual = ours(&feature);
    let gl = json!({
        "geometry": {"type": "MultiPolygon", "coordinates": []},
        "properties": {"name": "a"},
        "source": "zones",
        "state": {}
    });
    assert_eq!(
        theirs(&gl, &actual),
        actual,
        "Multi and the absent source layer match"
    );
    let with_id = json!({"geometry": {"type": "Polygon"}, "properties": {"name": "a"},
        "source": "zones", "id": 7});
    assert_ne!(
        theirs(&with_id, &actual),
        actual,
        "an id GL JS reports must be ours too"
    );
}
