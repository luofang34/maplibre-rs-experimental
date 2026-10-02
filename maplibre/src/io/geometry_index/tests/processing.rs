use geo_types::Point;
use geozero::{
    geojson::GeoJson, mvt::tile, ColumnValue, FeatureProcessor, GeomProcessor, GeozeroDatasource,
    PropertyProcessor,
};

use super::{multipoint_then_line, zigzag, IndexProcessor};
use crate::{
    coords::InnerCoords,
    io::geometry_index::{ExactGeometry, TileIndex},
};

fn name(properties: &crate::style::expression::FeatureProperties) -> Option<&str> {
    match properties.get("name") {
        Some(crate::style::expression::Value::String(text)) => Some(text.as_str()),
        _ => None,
    }
}

fn process_json(input: &str) -> IndexProcessor {
    let mut processor = IndexProcessor::new();
    GeoJson(input)
        .process(&mut processor)
        .expect("feature stream processes");
    processor
}

#[test]
fn a_feature_without_properties_is_queryable() {
    let processor = process_json(
        r#"{"type":"Feature","properties":null,"geometry":{"type":"LineString","coordinates":[[0,0],[100,0]]}}"#,
    );
    let index = TileIndex::Linear {
        list: processor.get_geometries(),
    };
    let hits = index.point_query(InnerCoords { x: 50.0, y: 0.0 });
    assert_eq!(hits.len(), 1);
    assert!(hits[0].properties.is_empty());
}

#[test]
fn a_features_properties_do_not_leak_into_the_next_feature() {
    let processor = process_json(
        r#"{"type":"FeatureCollection","features":[
        {"type":"Feature","properties":{"name":"point"},"geometry":{"type":"Point","coordinates":[0,0]}},
        {"type":"Feature","properties":null,"geometry":{"type":"LineString","coordinates":[[0,0],[100,0]]}}
    ]}"#,
    );
    let geometries = processor.get_geometries();
    assert_eq!(geometries.len(), 2);
    assert_eq!(name(&geometries[0].properties), Some("point"));
    assert!(geometries[1].properties.is_empty());
}

#[test]
fn empty_polygons_do_not_abort_later_features() {
    let processor = process_json(
        r#"{"type":"FeatureCollection","features":[
        {"type":"Feature","properties":{},"geometry":{"type":"Polygon","coordinates":[]}},
        {"type":"Feature","properties":{"name":"road"},"geometry":{"type":"LineString","coordinates":[[0,0],[100,0]]}}
    ]}"#,
    );
    let geometries = processor.get_geometries();
    assert_eq!(geometries.len(), 1);
    assert_eq!(name(&geometries[0].properties), Some("road"));
}

#[test]
fn empty_lines_do_not_enter_the_spatial_tree() {
    let processor = process_json(
        r#"{"type":"FeatureCollection","features":[
        {"type":"Feature","properties":{},"geometry":{"type":"LineString","coordinates":[]}},
        {"type":"Feature","properties":{},"geometry":{"type":"LineString","coordinates":[[0,0],[100,0]]}}
    ]}"#,
    );
    let tree = processor.build_tree();
    assert_eq!(tree.size(), 1);
    assert!(tree.nearest_neighbor(&Point::new(50.0, 0.0)).is_some());
}

#[test]
fn a_new_feature_discards_an_interrupted_geometry() {
    let mut processor = IndexProcessor::new();
    processor.feature_begin(0).expect("begin");
    processor.properties_begin().expect("properties");
    processor
        .property(0, "stale", &ColumnValue::Bool(true))
        .expect("property");
    processor.linestring_begin(true, 2, 0).expect("line");
    processor.xy(1.0, 1.0, 0).expect("first coordinate");
    // A layer decoder can return an error before it closes the current geometry.
    multipoint_then_line()
        .process(&mut processor)
        .expect("next layer processes");
    let geometries = processor.get_geometries();
    assert_eq!(
        geometries.len(),
        3,
        "the multi-point's two points and the line"
    );
    assert!(geometries.iter().all(|part| part.properties.is_empty()));
}

#[test]
fn every_mvt_line_part_keeps_its_properties_and_coordinate_scale() {
    let mut layer = multipoint_then_line();
    layer.features.remove(0);
    layer.features[0]
        .geometry
        .extend([9, zigzag(-100), zigzag(100), 10, zigzag(100), zigzag(0)]);
    layer.keys.push("name".to_string());
    layer.values.push(tile::Value {
        string_value: Some("road".to_string()),
        ..Default::default()
    });
    layer.features[0].tags = vec![0, 0];
    let mut processor = IndexProcessor::new();
    processor.set_coordinate_scale(0.5);
    layer
        .process(&mut processor)
        .expect("MVT with two line parts processes");
    let list = processor.get_geometries();
    assert_eq!(list.len(), 2);
    assert!(list.iter().all(|g| name(&g.properties) == Some("road")));
    let ExactGeometry::LineString(line) = &list[1].exact else {
        panic!("line expected")
    };
    assert_eq!(line.0, vec![(0.0, 100.0).into(), (50.0, 100.0).into()]);
    let index = TileIndex::Linear { list };
    assert_eq!(index.point_query(InnerCoords { x: 25.0, y: 25.0 }).len(), 1);
    assert_eq!(
        index.point_query(InnerCoords { x: 25.0, y: 100.0 }).len(),
        1
    );
}

#[test]
fn every_polygon_part_is_queryable_and_holes_stay_empty() {
    let processor = process_json(
        r#"{"type":"Feature","properties":{"name":"land"},"geometry":{
        "type":"MultiPolygon","coordinates":[
            [[[0,0],[100,0],[100,100],[0,100],[0,0]],[[20,20],[20,80],[80,80],[80,20],[20,20]]],
            [[[200,0],[300,0],[300,100],[200,100],[200,0]]]
        ]}}"#,
    );
    let index = TileIndex::Linear {
        list: processor.get_geometries(),
    };
    for x in [10.0, 250.0] {
        let hits = index.point_query(InnerCoords { x, y: 50.0 });
        assert_eq!(hits.len(), 1);
        assert_eq!(name(&hits[0].properties), Some("land"));
    }
    assert!(index
        .point_query(InnerCoords { x: 50.0, y: 50.0 })
        .is_empty());
}

#[test]
fn nested_collections_keep_queryable_parts_in_input_order() {
    let processor = process_json(
        r#"{"type":"Feature","properties":{"name":"mixed"},"geometry":{
        "type":"GeometryCollection","geometries":[
            {"type":"LineString","coordinates":[[0,0],[100,0]]},
            {"type":"GeometryCollection","geometries":[
                {"type":"Point","coordinates":[20,20]},
                {"type":"Polygon","coordinates":[[[0,100],[100,100],[100,200],[0,200],[0,100]]]}
            ]}
        ]}}"#,
    );
    let geometries = processor.get_geometries();
    assert_eq!(geometries.len(), 3);
    assert!(matches!(geometries[0].exact, ExactGeometry::LineString(_)));
    assert!(matches!(geometries[1].exact, ExactGeometry::Point(_)));
    assert!(matches!(geometries[2].exact, ExactGeometry::Polygon(_)));
    assert!(geometries
        .iter()
        .all(|g| name(&g.properties) == Some("mixed")));
}

#[test]
fn nonfinite_scaled_coordinates_return_an_error_and_allow_the_next_feature() {
    for (coordinate, scale) in [
        (f64::NAN, 1.0),
        (f64::INFINITY, 1.0),
        (f64::MAX, 2.0),
        (1.0, f64::NAN),
    ] {
        let mut processor = IndexProcessor::new();
        processor.feature_begin(0).expect("begin");
        processor.set_coordinate_scale(scale);
        processor.linestring_begin(true, 2, 0).expect("line");
        assert!(processor.xy(coordinate, 0.0, 0).is_err());
        processor.set_coordinate_scale(1.0);
        multipoint_then_line()
            .process(&mut processor)
            .expect("next layer processes");
        let parts = processor.get_geometries();
        assert_eq!(
            parts.len(),
            3,
            "the next layer's two points and line, and nothing else"
        );
        assert!(matches!(&parts[2].exact, ExactGeometry::LineString(line) if line.0.len() == 2));
    }
}

#[test]
fn an_object_property_stays_an_object() {
    let processor = process_json(
        r#"{"type":"Feature","properties":{"nested":{"inner":{"num":1}}},"geometry":{"type":"Point","coordinates":[0,0]}}"#,
    );
    let geometries = processor.get_geometries();
    assert_eq!(
        geometries[0]
            .properties
            .get("nested")
            .map(|value| value.to_json()),
        Some(serde_json::json!({"inner": {"num": 1}}))
    );
}
