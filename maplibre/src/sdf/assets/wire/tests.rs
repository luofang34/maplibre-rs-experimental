#![allow(clippy::expect_used)]

use std::sync::Arc;

use super::{decode_symbol_features, encode_symbol_features, SymbolFeature};
use crate::{
    euclid::{Box2D, Point2D},
    sdf::{placement_geometry::SymbolBounds, Feature, LineLabel, SymbolFeatureData},
    style::{
        expression::{FeatureProperties, Value},
        filter::GeometryType,
    },
};

#[test]
fn binary_symbol_wire_roundtrips_and_shares_polylines_and_repeat_keys() {
    let shared_poly: Arc<[[f32; 2]]> = vec![[0.0, 1.0], [10.0, 20.0], [30.0, 40.0]].into();
    let shared_key: Arc<str> = Arc::from("Prinsengracht");
    let mut props = FeatureProperties::default();
    props.insert("name".into(), Value::String("Prinsengracht".into()));
    props.insert("rank".into(), Value::Number(2.0));
    let props = Arc::new(props);

    let make_feature = |anchor_distance: f32, first_glyph: usize| Feature {
        parts: [
            Some(SymbolBounds {
                bounds: [-12.0, -6.0, 12.0, 6.0],
                height: 0.0,
                angle: 0.25,
                text: true,
            }),
            None,
            None,
        ],
        data: SymbolFeatureData {
            id: Some(42),
            properties: Arc::clone(&props),
            sort_key: 1.5,
            geometry_type: GeometryType::LineString,
            source_feature_index: Some(13),
        },
        bbox: Box2D::new(Point2D::new(-12.0, -6.0), Point2D::new(12.0, 6.0)),
        indices: first_glyph..(first_glyph + 18),
        text_anchor: Point2D::new(100.0, 200.0),
        str: "Prinsengracht".into(),
        line: Some(LineLabel {
            polyline: Arc::clone(&shared_poly),
            anchor_distance,
            glyph_offsets: vec![-8.0, 0.0, 8.0],
            image_sizes: Vec::new(),
            first_glyph_index: first_glyph,
        }),
        anchor_shifts: vec![[4.0, -4.0]],
        text_sets: vec![0..6, 6..12],
        anchor_sets: vec![0, 1],
        text_colors: vec![(0..6, [0.1, 0.2, 0.3, 1.0])],
        fallback: true,
        repeat_key: Some(Arc::clone(&shared_key)),
    };

    let input = vec![make_feature(15.0, 0), make_feature(45.0, 18)];
    let encoded = encode_symbol_features(&input);
    let json_bytes = serde_json::to_vec(&input.iter().map(SymbolFeature::from).collect::<Vec<_>>())
        .expect("json");
    assert!(
        encoded.len() < json_bytes.len(),
        "binary wire ({}) should be smaller than JSON ({})",
        encoded.len(),
        json_bytes.len()
    );

    let decoded = decode_symbol_features(&encoded).expect("valid binary symbol wire");
    assert_eq!(decoded.len(), 2);
    assert_eq!(decoded[0].str, "Prinsengracht");
    assert_eq!(decoded[0].data.id, Some(42));
    assert_eq!(decoded[0].data.source_feature_index, Some(13));
    assert_eq!(
        decoded[0].data.properties.get("name"),
        Some(&Value::String("Prinsengracht".into()))
    );
    assert_eq!(decoded[0].data.geometry_type, GeometryType::LineString);
    assert!(decoded[0].fallback);
    assert_eq!(decoded[0].anchor_shifts, vec![[4.0, -4.0]]);
    assert_eq!(decoded[0].text_sets, vec![0..6, 6..12]);
    assert_eq!(decoded[0].anchor_sets, vec![0, 1]);
    assert_eq!(decoded[0].text_colors, vec![(0..6, [0.1, 0.2, 0.3, 1.0])]);

    let poly0 = &decoded[0].line.as_ref().expect("line 0").polyline;
    let poly1 = &decoded[1].line.as_ref().expect("line 1").polyline;
    assert!(
        Arc::ptr_eq(poly0, poly1),
        "repeated road labels must share one Arc polyline after decode"
    );
    let rk0 = decoded[0].repeat_key.as_ref().expect("rk 0");
    let rk1 = decoded[1].repeat_key.as_ref().expect("rk 1");
    assert!(
        Arc::ptr_eq(rk0, rk1),
        "repeated road labels must share one Arc repeat_key after decode"
    );
    assert!(
        Arc::ptr_eq(&decoded[0].data.properties, &decoded[1].data.properties),
        "repeated road labels must share one Arc<FeatureProperties> after decode"
    );

    let legacy = decode_symbol_features(&json_bytes).expect("legacy JSON fallback");
    assert_eq!(legacy.len(), 2);
    assert_eq!(legacy[0].str, "Prinsengracht");
}
