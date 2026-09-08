use crate::style::expression::{FeatureProperties, Value};

#[test]
fn zoom_range_is_min_inclusive_max_exclusive() {
    let mut layer = super::StyleLayer {
        index: 0,
        id: "labels".to_string(),
        type_: "symbol".to_string(),
        filter: None,
        maxzoom: Some(6),
        minzoom: Some(2),
        metadata: None,
        paint: None,
        source: None,
        source_layer: None,
        visibility: super::LayerVisibility::Visible,
    };

    assert!(!layer.is_visible_at(1.99));
    assert!(layer.is_visible_at(2.0));
    assert!(layer.is_visible_at(5.99));
    assert!(!layer.is_visible_at(6.0));

    layer.minzoom = None;
    layer.maxzoom = None;
    assert!(layer.is_visible_at(0.0));
    assert!(layer.is_visible_at(24.0));
}

#[test]
fn a_layer_with_visibility_none_is_hidden_at_every_zoom() {
    let hidden: super::StyleLayer = serde_json::from_value(serde_json::json!({
        "id": "water", "type": "fill", "source": "s", "source-layer": "water",
        "layout": {"visibility": "none"}
    }))
    .expect("layer parses");
    assert!(hidden.is_hidden());
    assert!(!hidden.is_visible_at(0.0) && !hidden.is_visible_at(12.0));

    for layout in [
        serde_json::json!({"visibility": "visible"}),
        serde_json::json!({}),
    ] {
        let visible: super::StyleLayer = serde_json::from_value(serde_json::json!({
            "id": "water", "type": "fill", "source": "s", "source-layer": "water",
            "layout": layout
        }))
        .expect("layer parses");
        assert!(!visible.is_hidden());
        assert!(visible.is_visible_at(0.0));
    }
}

use super::*;

#[test]
fn test_evaluate_match_missing_property_returns_fallback() {
    let json = r#"
    [
        "match",
        ["get", "ADM0_A3"],
        ["ARM", "ATG"],
        "rgba(1, 2, 3, 1)",
        "rgba(9, 9, 9, 1)"
    ]
    "#;
    let expr: serde_json::Value = serde_json::from_str(json).unwrap();
    let prop: StyleProperty<csscolorparser::Color> = StyleProperty::parse(&expr);

    // Feature that does NOT have the property → should return the JSON fallback color
    let empty_props = FeatureProperties::new();
    let color = prop.evaluate_for(&empty_props, 0.0).unwrap();
    assert_eq!(color.to_rgba8(), [9, 9, 9, 255]);
}

#[test]
fn fill_and_line_layers_carry_their_opacity_property() {
    let fill: StyleLayer = serde_json::from_value(serde_json::json!({
        "id": "water", "type": "fill", "source": "s",
        "paint": {"fill-color": "#0000ff", "fill-opacity": 0.3}
    }))
    .expect("layer parses");
    let line: StyleLayer = serde_json::from_value(serde_json::json!({
        "id": "road", "type": "line", "source": "s",
        "paint": {"line-opacity": {"stops": [[0, 0.5], [1, 0.6]]}}
    }))
    .expect("layer parses");
    let plain: StyleLayer = serde_json::from_value(serde_json::json!({
        "id": "land", "type": "fill", "source": "s", "paint": {"fill-color": "#00ff00"}
    }))
    .expect("layer parses");

    let opacity = |layer: &StyleLayer| layer.paint.as_ref().and_then(LayerPaint::opacity);
    assert!(matches!(opacity(&fill), Some(StyleProperty::Constant(value)) if value == 0.3));
    assert!(
        opacity(&line)
            .expect("line opacity")
            .evaluate_at_zoom(0.5)
            .is_some_and(|value| (value - 0.55).abs() < 1e-6),
        "zoom functions evaluate at the zoom"
    );
    assert!(opacity(&plain).is_none());
}

#[test]
fn test_evaluate_match() {
    let json = r#"
    [
        "match",
        ["get", "ADM0_A3"],
        ["ARM", "ATG"],
        "rgba(1, 2, 3, 1)",
        "rgba(0, 0, 0, 1)"
    ]
    "#;
    let expr: serde_json::Value = serde_json::from_str(json).unwrap();
    let prop: StyleProperty<csscolorparser::Color> = StyleProperty::parse(&expr);

    let mut feature_properties = FeatureProperties::new();
    feature_properties.insert("ADM0_A3".to_string(), Value::from("ARM"));

    let color = prop.evaluate_for(&feature_properties, 0.0).unwrap();
    assert_eq!(color.to_rgba8(), [1, 2, 3, 255]);
}

#[test]
fn test_symbol_text_field_from_layout() {
    let json = r#"{
        "id": "countries-label",
        "type": "symbol",
        "paint": {
            "text-color": "rgba(8, 37, 77, 1)"
        },
        "layout": {
            "text-field": "{NAME}",
            "text-font": ["Open Sans Semibold"]
        },
        "source": "maplibre",
        "source-layer": "centroids"
    }"#;
    let layer: StyleLayer = serde_json::from_str(json).unwrap();
    assert_eq!(layer.type_, "symbol");
    match &layer.paint {
        Some(LayerPaint::Symbol(sp)) => {
            assert_eq!(text_of(sp, 3.0).as_deref(), Some("Berlin"));
        }
        other => panic!("expected Symbol paint, got {:?}", other),
    }
}

/// The text a symbol paint produces for a feature named `Berlin` and abbreviated `BER`.
fn text_of(paint: &SymbolPaint, zoom: f64) -> Option<String> {
    let properties = crate::style::expression::FeatureProperties::from([
        ("NAME".to_string(), Value::String("Berlin".to_string())),
        ("ABBREV".to_string(), Value::String("BER".to_string())),
    ]);
    paint
        .text_field
        .as_ref()?
        .evaluate_for(&properties, zoom)
        .map(|text| text.0)
}

#[test]
fn test_symbol_text_field_zoom_dependent() {
    let json = r#"{
        "id": "test-label",
        "type": "symbol",
        "paint": {},
        "layout": {
            "text-field": {"stops": [[2, "{ABBREV}"], [4, "{NAME}"]]}
        },
        "source": "maplibre",
        "source-layer": "centroids"
    }"#;
    let layer: StyleLayer = serde_json::from_str(json).unwrap();
    match &layer.paint {
        Some(LayerPaint::Symbol(sp)) => {
            assert_eq!(text_of(sp, 3.0).as_deref(), Some("BER"));
            assert_eq!(text_of(sp, 5.0).as_deref(), Some("Berlin"));
        }
        other => panic!("expected Symbol paint, got {:?}", other),
    }
}

#[test]
fn test_demotiles_symbol_layers_have_text_field() {
    let style: crate::style::Style = Default::default();
    for layer in &style.layers {
        if layer.type_ == "symbol" {
            match &layer.paint {
                Some(LayerPaint::Symbol(sp)) => {
                    assert!(
                        sp.text_field.is_some(),
                        "symbol layer '{}' should have text_field parsed from layout",
                        layer.id
                    );
                }
                _ => panic!("symbol layer '{}' has no Symbol paint", layer.id),
            }
        }
    }
}

#[test]
fn parses_fill_and_line_translation_properties() {
    let style: crate::style::Style = serde_json::from_str(
        r#"{
            "version": 8,
            "sources": {},
            "layers": [
                {
                    "id": "fill",
                    "type": "fill",
                    "paint": {
                        "fill-color": "red",
                        "fill-translate": [10, 50],
                        "fill-translate-anchor": "viewport"
                    }
                },
                {
                    "id": "line",
                    "type": "line",
                    "paint": {
                        "line-color": "blue",
                        "line-translate": [2, 3]
                    }
                }
            ]
        }"#,
    )
    .unwrap();

    let Some(LayerPaint::Fill(fill)) = style.layers[0].paint.as_ref() else {
        panic!("first layer should be a fill");
    };
    assert_eq!(fill.fill_translate, Some([10.0, 50.0]));
    assert_eq!(fill.fill_translate_anchor, TranslateAnchor::Viewport);

    let Some(LayerPaint::Line(line)) = style.layers[1].paint.as_ref() else {
        panic!("second layer should be a line");
    };
    assert_eq!(line.line_translate, Some([2.0, 3.0]));
    assert_eq!(line.line_translate_anchor, TranslateAnchor::Map);
}
