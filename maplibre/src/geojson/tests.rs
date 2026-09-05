use geozero::GeozeroDatasource;

use super::ProjectingTessellator;
use crate::{
    coords::WorldTileCoords,
    vector::tessellation::{IndexDataType, ZeroTessellator},
};

#[test]
fn globe_geojson_world_polygon_generates_subdivided_triangles() {
    let geometry = r#"{
        "type": "Polygon",
        "coordinates": [[
            [-180, -90], [-180, 90], [180, 90], [180, -90], [-180, -90]
        ]]
    }"#;
    let tessellator =
        ZeroTessellator::<IndexDataType>::default().with_globe_subdivision(32, true, true, true);
    let mut projecting = ProjectingTessellator::new(WorldTileCoords::default(), tessellator);
    let mut source = geozero::geojson::GeoJson(geometry);

    source
        .process(&mut projecting)
        .expect("world polygon should tessellate");
    let tessellator = projecting.into_inner();

    assert!(tessellator.buffer.vertices.len() > 4);
    assert!(tessellator.buffer.indices.len() > 6);
}

#[test]
fn layer_filters_apply_to_geojson_features() {
    use serde_json::json;

    use crate::{geojson::filter_geojson, style::filter::Filter};

    let collection = json!({
        "type": "FeatureCollection",
        "features": [
            {"type": "Feature", "properties": {"level": "low"},
             "geometry": {"type": "LineString", "coordinates": [[0, 0], [1, 1]]}},
            {"type": "Feature", "properties": {"level": "high"},
             "geometry": {"type": "LineString", "coordinates": [[0, 0], [2, 2]]}},
            {"type": "Feature", "properties": {},
             "geometry": {"type": "Point", "coordinates": [0, 0]}}
        ]
    });
    let filter = Filter::parse(&json!(["==", ["get", "level"], "low"])).expect("filter parses");
    let filtered = filter_geojson(&collection, &filter, 3.0);
    assert_eq!(filtered["features"].as_array().map(Vec::len), Some(1));
    assert_eq!(filtered["features"][0]["properties"]["level"], json!("low"));

    let by_type = Filter::parse(&json!(["==", "$type", "Point"])).expect("filter parses");
    let filtered = filter_geojson(&collection, &by_type, 3.0);
    assert_eq!(filtered["features"].as_array().map(Vec::len), Some(1));

    let single = json!({"type": "Feature", "properties": {"level": "high"},
        "geometry": {"type": "LineString", "coordinates": [[0, 0], [2, 2]]}});
    assert_eq!(
        filter_geojson(&single, &filter, 3.0)["features"]
            .as_array()
            .map(Vec::len),
        Some(0)
    );
    let bare = json!({"type": "Point", "coordinates": [0, 0]});
    assert_eq!(filter_geojson(&bare, &by_type, 3.0), bare);
}
