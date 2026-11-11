use super::*;
use crate::coords::InnerCoords;

#[test]
fn spatial_and_linear_roundtrip_preserve_holes_and_shared_property_identity() {
    let mut shared_props = FeatureProperties::new();
    shared_props.insert("name".into(), Value::String("courtyard".into()));
    shared_props.insert("floors".into(), Value::Number(5.0));
    shared_props.insert("open".into(), Value::Bool(true));
    shared_props.insert(
        "tags".into(),
        Value::Array(vec![Value::String("a".into()), Value::Null]),
    );
    let shared_arc = Arc::new(shared_props);

    let outer = LineString::from(vec![
        (100.0, 100.0),
        (500.0, 100.0),
        (500.0, 500.0),
        (100.0, 500.0),
        (100.0, 100.0),
    ]);
    let hole = LineString::from(vec![
        (200.0, 200.0),
        (300.0, 200.0),
        (300.0, 300.0),
        (200.0, 300.0),
        (200.0, 200.0),
    ]);
    let part_a = IndexedGeometry::from_polygon(
        Polygon::new(outer, vec![hole]),
        FeatureMeta {
            properties: shared_arc.clone(),
            source_layer: Arc::from("building"),
            id: None,
            feature_index: 7,
        },
    )
    .unwrap();
    let part_b = IndexedGeometry::from_linestring(
        LineString::from(vec![(600.0, 600.0), (800.0, 600.0)]),
        FeatureMeta {
            properties: shared_arc,
            source_layer: Arc::from("building"),
            id: None,
            feature_index: 7,
        },
    )
    .unwrap();
    let pt = IndexedGeometry::from_point(
        Point::new(50.0, 50.0),
        FeatureMeta {
            properties: Arc::new(FeatureProperties::new()),
            source_layer: Arc::from("poi"),
            id: Some(42),
            feature_index: 1,
        },
    )
    .unwrap();

    let linear = TileIndex::Linear {
        list: vec![part_a.clone(), part_b.clone(), pt.clone()],
    };
    let wire_linear = WireTileIndex::from_tile_index(&linear);
    assert!(!wire_linear.is_spatial);
    assert_eq!(wire_linear.strings.len(), 2);
    assert_eq!(wire_linear.prop_offsets.len(), 3); // 2 unique property groups + sentinel

    let decoded_linear = wire_linear.into_tile_index();
    let TileIndex::Linear { list } = &decoded_linear else {
        panic!("expected Linear TileIndex");
    };
    assert_eq!(list.len(), 3);
    assert!(Arc::ptr_eq(&list[0].properties, &list[1].properties));
    assert!(!Arc::ptr_eq(&list[0].properties, &list[2].properties));
    assert_eq!(list[2].id, Some(42));
    assert_eq!(list[0].properties, part_a.properties);

    let spatial = TileIndex::Spatial {
        tree: RTree::bulk_load(vec![part_a, part_b, pt]),
    };
    let decoded_spatial = WireTileIndex::from_tile_index(&spatial).into_tile_index();
    assert!(matches!(decoded_spatial, TileIndex::Spatial { .. }));
    // Inside exterior, outside hole -> hits polygon.
    assert_eq!(
        decoded_spatial
            .point_query(InnerCoords { x: 150.0, y: 150.0 })
            .len(),
        1
    );
    // Inside hole -> misses polygon.
    assert!(decoded_spatial
        .point_query(InnerCoords { x: 250.0, y: 250.0 })
        .is_empty());
    // Within 8 units of line -> hits line.
    assert_eq!(
        decoded_spatial
            .point_query(InnerCoords { x: 700.0, y: 604.0 })
            .len(),
        1
    );
}
