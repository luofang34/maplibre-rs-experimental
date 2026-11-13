use super::*;
use crate::{
    coords::InnerCoords,
    style::{
        expression::FEATURE_STATE_PREFIX,
        feature_state::{
            extract_feature_state, extract_source_properties, sync_properties_feature_state,
            FeatureId, FeatureStateStore, FeatureStateTarget, STATE_ID_PROPERTY,
        },
    },
};

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

#[test]
fn wire_roundtrip_preserves_complex_values_and_feature_state_synchronization() {
    let mut nested_obj = BTreeMap::new();
    nested_obj.insert("tier".to_string(), Value::Number(2.0));
    nested_obj.insert(
        "badges".to_string(),
        Value::Array(vec![Value::String("verified".into()), Value::Null]),
    );

    let mut props = FeatureProperties::new();
    props.insert("osm_id".into(), Value::String("way/99".into()));
    props.insert(
        "tint".into(),
        Value::Color(Color::new(0.25, 0.5, 0.75, 0.8)),
    );
    props.insert("meta".into(), Value::Object(nested_obj));
    props.insert(STATE_ID_PROPERTY.into(), Value::String("way/99".into()));
    props.insert(format!("{FEATURE_STATE_PREFIX}hover"), Value::Bool(true));
    props.insert(
        format!("{FEATURE_STATE_PREFIX}weights"),
        Value::Array(vec![Value::Number(1.5), Value::Bool(false)]),
    );

    let geom = IndexedGeometry::from_linestring(
        LineString::from(vec![(200.0, 512.0), (300.0, 512.0)]),
        FeatureMeta {
            properties: Arc::new(props.clone()),
            source_layer: Arc::from("roads"),
            id: Some(99),
            feature_index: 3,
        },
    )
    .expect("valid linestring");

    let spatial = TileIndex::Spatial {
        tree: RTree::bulk_load(vec![geom]),
    };
    let mut decoded = WireTileIndex::from_tile_index(&spatial).into_tile_index();
    let hits = decoded.point_query(InnerCoords { x: 256.0, y: 512.0 });
    assert_eq!(hits.len(), 1);
    assert_eq!(*hits[0].properties, props);

    // Internal state keys stay hidden from source properties while remaining readable as state.
    let source_props = extract_source_properties(&hits[0].properties);
    assert!(!source_props.contains_key(STATE_ID_PROPERTY));
    assert!(!source_props
        .keys()
        .any(|k| k.starts_with(FEATURE_STATE_PREFIX)));
    assert_eq!(
        source_props.get("osm_id"),
        Some(&serde_json::json!("way/99"))
    );
    let state_before = extract_feature_state(&hits[0].properties);
    assert_eq!(state_before.get("hover"), Some(&serde_json::json!(true)));
    assert_eq!(
        state_before.get("weights"),
        Some(&serde_json::json!([1.5, false]))
    );

    // Subsequent runtime `set_feature_state` updates mutate the decoded TileIndex in place.
    let mut store = FeatureStateStore::default();
    let mut next_state = serde_json::Map::new();
    next_state.insert("hover".into(), serde_json::Value::Null);
    next_state.insert("selected".into(), serde_json::json!("primary"));
    store.set(
        FeatureStateTarget::vector("composite", "roads"),
        FeatureId::from("way/99"),
        next_state,
    );

    let TileIndex::Spatial { tree } = &mut decoded else {
        panic!("expected spatial index");
    };
    let mut items: Vec<IndexedGeometry<f64>> = std::mem::take(tree).into_iter().collect();
    let item_id = items[0].id;
    assert!(sync_properties_feature_state(
        Arc::make_mut(&mut items[0].properties),
        item_id,
        Some("osm_id"),
        store.source("composite"),
        Some("roads"),
    ));
    let state_after = extract_feature_state(&items[0].properties);
    assert!(!state_after.contains_key("hover"));
    assert_eq!(
        state_after.get("selected"),
        Some(&serde_json::json!("primary"))
    );
}
