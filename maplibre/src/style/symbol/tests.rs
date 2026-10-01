#![allow(clippy::expect_used, clippy::panic)]
use super::*;

#[test]
fn labels_preserve_unicode_and_apply_transform_before_trimming() {
    for (input, transform, expected) in [
        ("  مرحبًا 🌍  ", "none", Some("مرحبًا 🌍")),
        ("  Straße  ", "uppercase", Some("STRASSE")),
        ("  ALPS  ", "lowercase", Some("alps")),
        (" \t\n ", "none", None),
    ] {
        let paint: SymbolPaint = serde_json::from_value(serde_json::json!({
            "text-field": input, "text-transform": transform
        }))
        .expect("paint");
        assert_eq!(
            paint.label(&FeatureProperties::new(), 0.0).as_deref(),
            expected
        );
    }
}

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

#[test]
fn a_format_expression_gives_its_sections_their_own_size_and_colour() {
    let paint: SymbolPaint = serde_json::from_value(serde_json::json!({
        "text-field": ["format", "Big", {"font-scale": 1.5, "text-color": "#ff0000"}, "\n", {}, ["get", "name"], {}]
    }))
    .expect("paint");
    let properties = FeatureProperties::from([("name".to_string(), "small".into())]);
    let sections = paint.label_sections(&properties, 0.0);
    assert_eq!(paint.label(&properties, 0.0).as_deref(), Some("Big\nsmall"));
    assert_eq!(sections.len(), 3);
    assert_eq!(sections[0].scale, Some(1.5));
    assert_eq!(sections[0].color.map(|color| color[0]), Some(1.0));
    assert_eq!(sections[2].length, "small".chars().count());
}

#[test]
fn a_format_inside_a_match_keeps_its_sections() {
    let paint: SymbolPaint = serde_json::from_value(serde_json::json!({
        "text-field": ["match", ["get", "case"], "false", "error", "one",
            ["format", "Green", {"text-color": "green"}, "Two", {}], "default"]
    }))
    .expect("paint");
    let properties = FeatureProperties::from([("case".to_string(), "one".into())]);
    assert_eq!(paint.label(&properties, 0.0).as_deref(), Some("GreenTwo"));
    assert_eq!(paint.label_sections(&properties, 0.0).len(), 2);
}

#[test]
fn an_image_in_a_format_is_a_placeholder_character_with_its_own_section() {
    let paint: SymbolPaint = serde_json::from_value(serde_json::json!({
        "text-field": ["format", "Stop ", {}, ["image", "bus"], {"font-scale": 2.0}]
    }))
    .expect("paint");
    let properties = FeatureProperties::new();
    let sections = paint.label_sections(&properties, 0.0);

    assert_eq!(
        paint.label(&properties, 0.0).as_deref(),
        Some("Stop \u{e000}")
    );
    assert_eq!(sections.len(), 2);
    assert_eq!(sections[0].image, None);
    assert_eq!(sections[1].image.as_deref(), Some("bus"));
    assert_eq!(sections[1].scale, Some(2.0));
    assert_eq!(sections[1].length, 1);
}
