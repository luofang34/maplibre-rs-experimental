use std::sync::Arc;

use serde_json::json;

use super::{collect_layer_referenced_keys, symbol_runtime_referenced_keys};
use crate::style::{
    layer::{LayerPaint, LinePaint, StyleLayer, SymbolPaint},
    property::{PropertyExpression, PropertyValue, StyleProperty},
};

fn worker_expr<T: PropertyValue>(expr: serde_json::Value) -> StyleProperty<T> {
    let StyleProperty::Expression(parsed) = StyleProperty::<T>::parse(&expr) else {
        panic!("expected expression property");
    };
    StyleProperty::Expression(Arc::new(PropertyExpression::from_worker_expression(
        parsed.expression().clone(),
    )))
}

#[test]
fn walks_worker_ast_expressions_with_null_source_and_unrecognized_layout() {
    let mut layer = StyleLayer {
        id: "roads".to_owned(),
        type_: "line".to_owned(),
        filter: Some(json!([
            "all",
            ["==", "class", "primary"],
            ["has", "oneway"]
        ])),
        paint: Some(LayerPaint::Line(LinePaint {
            line_color: Some(worker_expr(json!([
                "case",
                ["has", "bridge"],
                "#ff0000",
                ["==", ["get", "tunnel"], 1],
                "#00ff00",
                "#0000ff"
            ]))),
            ..Default::default()
        })),
        ..Default::default()
    };
    layer
        .unrecognized
        .layout
        .insert("line-cap".to_owned(), json!("round"));
    layer
        .unrecognized
        .layout
        .insert("line-sort-key".to_owned(), json!(["get", "rank"]));

    let keys = collect_layer_referenced_keys(&[&layer], Some("osm_id")).expect("static keys");
    for expected in ["osm_id", "class", "oneway", "bridge", "tunnel", "rank"] {
        assert!(keys.contains(expected), "missing key {expected}");
    }
    assert_eq!(keys.len(), 6);
}

#[test]
fn handles_token_strings_legacy_functions_and_sub_object_gets() {
    let mut props = serde_json::Map::new();
    props.insert(
        "icon-image".to_owned(),
        json!({"stops": [[5, "{maki}_11"], [10, "dot"]]}),
    );
    props.insert(
        "text-color".to_owned(),
        json!({"property": "kind", "stops": [["city", "#000"], ["town", "#333"]]}),
    );
    let layer = StyleLayer {
        id: "labels".to_owned(),
        type_: "symbol".to_owned(),
        paint: Some(LayerPaint::Symbol(SymbolPaint {
            text_field: Some(worker_expr(json!("{name:latin} ({code})"))),
            text_size: Some(worker_expr(json!(["get", "sub", ["get", "meta"]]))),
            properties: props,
            ..Default::default()
        })),
        ..Default::default()
    };

    let all = collect_layer_referenced_keys(&[&layer], None).expect("static keys");
    for expected in ["name:latin", "code", "maki", "kind", "meta"] {
        assert!(all.contains(expected), "missing decode key {expected}");
    }
    assert!(
        !all.contains("sub"),
        "nested sub-object key is not a top-level MVT property"
    );

    let runtime = symbol_runtime_referenced_keys(&layer, Some("fid")).expect("runtime keys");
    assert!(runtime.contains("kind"));
    assert!(runtime.contains("meta"));
    assert!(runtime.contains("fid"));
    assert!(!runtime.contains("name:latin"));
    assert!(!runtime.contains("code"));
    assert!(!runtime.contains("maki"));
}

#[test]
fn bails_out_to_none_on_dynamic_get_or_properties_operator() {
    let dynamic_layer = StyleLayer {
        id: "dyn".to_owned(),
        type_: "line".to_owned(),
        paint: Some(LayerPaint::Line(LinePaint {
            line_width: Some(worker_expr(json!([
                "number",
                ["get", ["get", "key_name"]],
                1.0
            ]))),
            ..Default::default()
        })),
        ..Default::default()
    };
    assert!(collect_layer_referenced_keys(&[&dynamic_layer], None).is_none());

    let props_layer = StyleLayer {
        id: "all_props".to_owned(),
        type_: "fill".to_owned(),
        filter: Some(json!(["has", "x", ["properties"]])),
        ..Default::default()
    };
    assert!(collect_layer_referenced_keys(&[&props_layer], None).is_none());
}

#[test]
fn symbol_runtime_keys_capture_icon_text_fit_and_height_offset() {
    let mut props = serde_json::Map::new();
    props.insert("icon-text-fit".to_owned(), json!(["get", "fit_mode"]));
    props.insert("symbol-height-offset".to_owned(), json!(["get", "elev_m"]));
    props.insert("text-font".to_owned(), json!(["get", "font_stack"]));
    let layer = StyleLayer {
        id: "shields".to_owned(),
        type_: "symbol".to_owned(),
        filter: Some(json!(["==", ["get", "shield_class"], "interstate"])),
        paint: Some(LayerPaint::Symbol(SymbolPaint {
            properties: props,
            ..Default::default()
        })),
        ..Default::default()
    };

    let runtime = symbol_runtime_referenced_keys(&layer, None).expect("runtime keys");
    assert!(runtime.contains("fit_mode"));
    assert!(runtime.contains("elev_m"));
    assert!(!runtime.contains("font_stack"));
    assert!(!runtime.contains("shield_class"));
}
