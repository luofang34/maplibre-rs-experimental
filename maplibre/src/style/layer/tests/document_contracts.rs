#![allow(clippy::expect_used)]

use crate::style::{layer::StyleLayer, Style};

#[test]
fn fractional_zoom_thresholds_control_visibility_without_rounding() {
    let layer: StyleLayer = serde_json::from_value(serde_json::json!({
        "id": "land", "type": "fill", "minzoom": 12.125, "maxzoom": 12.15,
    }))
    .expect("fractional layer zoom");
    assert!(!layer.is_visible_at(12.124));
    assert!(layer.is_visible_at(12.125));
    assert!(layer.is_visible_at(12.149));
    assert!(!layer.is_visible_at(12.15));
}

#[test]
fn fractional_zoom_limits_survive_the_worker_style_roundtrip() {
    let document = serde_json::json!({"version":8, "sources":{}, "layers":[
        {"id":"land", "type":"fill", "minzoom":12.125, "maxzoom":12.15}
    ]});
    let style: Style = serde_json::from_value(document).expect("style");
    let encoded = serde_json::to_value(&style).expect("worker payload");
    assert_eq!(encoded["layers"][0]["minzoom"], 12.125);
    assert_eq!(encoded["layers"][0]["maxzoom"], 12.15);
    let decoded: Style = serde_json::from_value(encoded).expect("worker style");
    assert!(decoded.layers[0].is_visible_at(12.14));
    assert!(!decoded.layers[0].is_visible_at(12.15));
}

#[test]
fn layer_metadata_preserves_nested_json_values_without_affecting_visibility() {
    let metadata = serde_json::json!({
        "app:priority": 3, "app:enabled": true, "app:tags": ["water", 4, null],
        "app:details": {"scale": 2.5, "note": "map"}, "app:empty": null,
    });
    let layer: StyleLayer = serde_json::from_value(serde_json::json!({
        "id":"land", "type":"fill", "metadata": metadata,
    }))
    .expect("JSON metadata");
    let encoded = serde_json::to_value(&layer).expect("layer document");
    assert_eq!(encoded["metadata"], metadata);
    let decoded: StyleLayer = serde_json::from_value(encoded).expect("layer roundtrip");
    for zoom in [0.0, 12.14, 24.0] {
        assert!(decoded.is_visible_at(zoom));
    }
}
