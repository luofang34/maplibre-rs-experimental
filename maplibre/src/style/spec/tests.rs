#![allow(clippy::expect_used, clippy::panic)]

use std::str::FromStr;

use serde_json::Value;

use super::*;
use crate::style::expression::PropertyKind;

#[test]
fn generated_tables_match_every_paint_and_layout_table_in_style_spec_v8_json() {
    let spec_json: Value = serde_json::from_str(include_str!("../../../style-spec-v8.json"))
        .expect("style-spec-v8.json parses");
    let root = spec_json.as_object().expect("root object");

    let paint_tables: Vec<&String> = root["paint"]
        .as_array()
        .expect("paint list")
        .iter()
        .map(|v| v.as_str().expect("table name"))
        .map(|s| root.get_key_value(s).expect("table exists").0)
        .collect();
    assert_eq!(paint_tables.len(), 10);

    for table_key in paint_tables {
        let layer_name = table_key.strip_prefix("paint_").expect("paint_ prefix");
        let layer_type = SpecLayerType::from_str(layer_name).expect("known layer type");
        let json_props = root[table_key].as_object().expect("paint table object");
        assert_eq!(
            layer_type.paint_properties().len(),
            json_props.len(),
            "paint property count mismatch for `{layer_name}`"
        );
        for (prop_name, prop_spec) in json_props {
            let meta = lookup_paint_property(layer_name, prop_name)
                .unwrap_or_else(|| panic!("missing paint property `{layer_name}.{prop_name}`"));
            assert_eq!(meta.name, prop_name);
            assert_eq!(meta.layer_type, layer_type);
            assert_eq!(meta.kind, SpecPropertyKind::Paint);
            assert_eq!(
                meta.supports_transition,
                prop_spec.get("transition").and_then(Value::as_bool) == Some(true),
                "transition mismatch on `{layer_name}.{prop_name}`"
            );
            assert_eq!(
                meta.supports_tokens,
                prop_spec.get("tokens").and_then(Value::as_bool) == Some(true),
                "tokens mismatch on `{layer_name}.{prop_name}`"
            );
            assert_eq!(
                meta.default_value().as_ref(),
                prop_spec.get("default"),
                "default mismatch on `{layer_name}.{prop_name}`"
            );
        }
    }

    let layout_tables: Vec<&String> = root["layout"]
        .as_array()
        .expect("layout list")
        .iter()
        .map(|v| v.as_str().expect("table name"))
        .map(|s| root.get_key_value(s).expect("table exists").0)
        .collect();
    assert_eq!(layout_tables.len(), 10);

    for table_key in layout_tables {
        let layer_name = table_key.strip_prefix("layout_").expect("layout_ prefix");
        let layer_type = SpecLayerType::from_str(layer_name).expect("known layer type");
        let json_props = root[table_key].as_object().expect("layout table object");
        assert_eq!(
            layer_type.layout_properties().len(),
            json_props.len(),
            "layout property count mismatch for `{layer_name}`"
        );
        for (prop_name, prop_spec) in json_props {
            let meta = lookup_layout_property(layer_name, prop_name)
                .unwrap_or_else(|| panic!("missing layout property `{layer_name}.{prop_name}`"));
            assert_eq!(meta.name, prop_name);
            assert_eq!(meta.layer_type, layer_type);
            assert_eq!(meta.kind, SpecPropertyKind::Layout);
            assert_eq!(
                meta.supports_tokens,
                prop_spec.get("tokens").and_then(Value::as_bool) == Some(true),
                "tokens mismatch on `{layer_name}.{prop_name}`"
            );
            assert_eq!(
                meta.default_value().as_ref(),
                prop_spec.get("default"),
                "default mismatch on `{layer_name}.{prop_name}`"
            );
        }
    }
}

#[test]
fn typed_property_enums_round_trip_and_expose_expression_descriptors() {
    for &layer in SpecLayerType::ALL {
        assert_eq!(SpecLayerType::from_str(layer.as_str()), Ok(layer));
    }
    for &prop in SymbolLayoutProperty::ALL {
        assert_eq!(SymbolLayoutProperty::from_str(prop.as_str()), Ok(prop));
        assert_eq!(prop.metadata().name, prop.as_str());
    }

    let fill_color = FillPaintProperty::FillColor.metadata();
    assert_eq!(fill_color.value_type, SpecValueType::Color);
    assert!(fill_color.expression.supports_interpolated);
    assert!(fill_color.expression.supports_piecewise_constant);
    assert!(fill_color.supports_transition);
    assert_eq!(
        fill_color.expression.parameters,
        &[
            SpecExpressionParameter::Zoom,
            SpecExpressionParameter::Feature,
            SpecExpressionParameter::FeatureState,
        ]
    );

    let line_gradient = LinePaintProperty::LineGradient.metadata();
    assert_eq!(
        line_gradient.expression.parameters,
        &[SpecExpressionParameter::LineProgress]
    );

    let heatmap_color = HeatmapPaintProperty::HeatmapColor.metadata();
    assert_eq!(
        heatmap_color.expression.parameters,
        &[SpecExpressionParameter::HeatmapDensity]
    );

    let sky_gradient = SkyPaintProperty::SkyGradient.metadata();
    assert_eq!(
        sky_gradient.expression.parameters,
        &[SpecExpressionParameter::SkyRadialProgress]
    );

    let text_field = SymbolLayoutProperty::TextField.metadata();
    assert_eq!(text_field.value_type, SpecValueType::Formatted);
    assert!(text_field.supports_tokens);
    assert!(!text_field.expression.supports_interpolated);
    assert!(text_field.expression.supports_piecewise_constant);
    assert_eq!(
        text_field.legacy_property_spec().kind,
        PropertyKind::Formatted
    );

    let line_cap = LineLayoutProperty::LineCap.metadata();
    assert_eq!(line_cap.value_type, SpecValueType::Enum);
    assert_eq!(line_cap.enum_values, &["butt", "round", "square"]);

    assert!(lookup_paint_property("fill", "nonexistent").is_none());
    assert!(lookup_layout_property("unknown-layer", "visibility").is_none());
}
