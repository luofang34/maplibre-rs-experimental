#![allow(clippy::expect_used, clippy::panic)]
use super::*;

#[test]
fn shared_symbol_height_evaluates_features_and_overrides_component_aliases() {
    let paint: SymbolPaint = serde_json::from_value(serde_json::json!({
        "symbol-height-offset": ["+", ["get", "altitude"], 50],
        "symbol-height-anchor": "absolute",
        "text-height-offset": 999, "icon-height-offset": 888,
        "text-height-anchor": "ground", "icon-height-anchor": "ground"
    }))
    .expect("paint");
    let properties = [(
        "altitude".into(),
        crate::style::expression::Value::Number(1200.0),
    )]
    .into();
    for component in ["text", "icon"] {
        assert_eq!(paint.height_offset(component, &properties, 12.0), 1250.0);
        assert!(!paint.height_follows_ground(component));
    }
}

#[test]
fn shared_symbol_height_interpolates_zoom_and_defaults_to_ground() {
    let paint: SymbolPaint = serde_json::from_value(serde_json::json!({
        "symbol-height-offset": ["interpolate", ["linear"], ["zoom"], 10, 0, 14, 400]
    }))
    .expect("paint");
    assert_eq!(
        paint.height_offset("text", &FeatureProperties::new(), 12.0),
        200.0
    );
    assert!(paint.height_follows_ground("icon"));
}

#[test]
fn component_height_aliases_remain_usable_without_shared_properties() {
    let paint: SymbolPaint = serde_json::from_value(serde_json::json!({
        "text-height-offset": 12, "icon-height-offset": 34, "text-height-anchor": "sea"
    }))
    .expect("paint");
    assert_eq!(
        paint.height_offset("text", &FeatureProperties::new(), 12.0),
        12.0
    );
    assert_eq!(
        paint.height_offset("icon", &FeatureProperties::new(), 12.0),
        34.0
    );
    assert!(!paint.height_follows_ground("text"));
    assert!(paint.height_follows_ground("icon"));
}
