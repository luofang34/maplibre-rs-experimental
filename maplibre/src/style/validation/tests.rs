#![allow(clippy::expect_used, clippy::panic)]

use super::StyleValidationError;
use crate::style::{filter::FilterError, Style};

#[test]
fn unsupported_filters_are_reported_per_layer() {
    let style: Style = serde_json::from_str(
        r#"{
            "version": 8,
            "sources": {},
            "layers": [
                {"id": "ok", "type": "line", "filter": ["==", ["get", "level"], "low"]},
                {"id": "bad", "type": "line", "filter": ["within", {"type": "Polygon", "coordinates": []}]}
            ]
        }"#,
    )
    .expect("style parses");

    let errors = style.validate().expect_err("the within filter is rejected");
    assert_eq!(errors.len(), 1);
    assert!(
        matches!(
            &errors[0],
            StyleValidationError::Filter { layer, source: FilterError::Invalid { .. } } if layer == "bad"
        ),
        "{errors:?}"
    );
}

#[test]
fn the_default_style_validates() {
    assert_eq!(Style::default().validate(), Ok(()));
}

fn style_with_layer(layer: serde_json::Value) -> Style {
    serde_json::from_value(serde_json::json!({
        "version": 8, "sources": {}, "layers": [layer]
    }))
    .expect("style parses")
}

#[test]
fn ignored_paint_and_layout_properties_are_reported() {
    let style = style_with_layer(serde_json::json!({
        "id": "roads", "type": "line",
        "paint": {"line-color": "red", "line-gradient": "blue"},
        "layout": {"line-cap": "round"}
    }));
    let errors = style
        .validate()
        .expect_err("ignored properties must be reported");
    assert_eq!(errors.len(), 2, "{errors:?}");
    for path in ["paint.line-gradient", "layout.line-cap"] {
        assert!(
            errors.iter().any(|error| {
                let text = error.to_string();
                text.contains("roads") && text.contains(path)
            }),
            "missing {path}: {errors:?}"
        );
    }
}

#[test]
fn unsupported_property_expressions_include_the_layer_and_property() {
    let style = style_with_layer(serde_json::json!({
        "id": "land", "type": "fill", "paint": {"fill-color": ["not-an-operator"]}
    }));
    let errors = style
        .validate()
        .expect_err("unsupported expression must be reported");
    assert!(errors[0].to_string().contains("land"));
    assert!(errors[0].to_string().contains("paint.fill-color"));
    assert!(std::error::Error::source(&errors[0]).is_some());
}

#[test]
fn raster_adjustments_and_unknown_layer_types_are_reported() {
    for (kind, paint, path) in [
        (
            "raster",
            serde_json::json!({"raster-opacity": 0.5}),
            "paint.raster-opacity",
        ),
        ("heatmap", serde_json::json!({}), "heatmap"),
    ] {
        let style = style_with_layer(serde_json::json!({
            "id": "unsupported", "type": kind, "paint": paint
        }));
        let errors = style
            .validate()
            .expect_err("unimplemented rendering must be reported");
        assert!(
            errors.iter().any(|error| error.to_string().contains(path)),
            "{errors:?}"
        );
    }
}

#[test]
fn malformed_paint_does_not_discard_the_whole_paint_silently() {
    let result = serde_json::from_value::<Style>(serde_json::json!({
        "version": 8, "sources": {}, "layers": [{
            "id": "roads", "type": "line", "paint": {"line-translate": "invalid"}
        }]
    }));
    let error = result
        .expect_err("malformed paint must fail to load")
        .to_string();
    assert!(
        error.contains("roads") && error.contains("paint"),
        "{error}"
    );
}

#[test]
fn unsupported_fields_and_hidden_symbols_survive_round_trips() {
    let style = style_with_layer(serde_json::json!({
        "id": "labels", "type": "symbol",
        "paint": {"text-color": "red", "text-translate": [2, 3]},
        "layout": {"text-field": "{name}", "visibility": "none", "text-variable-anchor": ["top"]}
    }));
    let encoded = serde_json::to_value(&style).expect("style serializes");
    let layer = &encoded["layers"][0];
    assert_eq!(layer["layout"]["text-field"], "{name}");
    assert_eq!(layer["layout"]["visibility"], "none");
    assert_eq!(
        layer["layout"]["text-variable-anchor"],
        serde_json::json!(["top"])
    );
    assert_eq!(layer["paint"]["text-translate"], serde_json::json!([2, 3]));
    let reloaded: Style = serde_json::from_value(encoded).expect("round trip parses");
    assert!(reloaded.layers[0].is_hidden());
    assert_eq!(style.validate(), reloaded.validate());
}

#[test]
fn validation_observes_programmatic_property_changes() {
    use crate::style::layer::{LayerPaint, StyleProperty};
    let mut style = style_with_layer(serde_json::json!({
        "id": "land", "type": "fill", "paint": {"fill-color": ["not-an-operator"]}
    }));
    assert!(style.validate().is_err());
    let Some(LayerPaint::Fill(paint)) = &mut style.layers[0].paint else {
        panic!("fill paint");
    };
    paint.fill_color = Some(StyleProperty::parse(&serde_json::json!("green")));
    assert_eq!(style.validate(), Ok(()));
}

#[test]
fn expression_diagnostics_follow_the_renderers_evaluation_context() {
    let style = style_with_layer(serde_json::json!({
        "id": "roads", "type": "line", "paint": {
            "line-width": ["get", "width"],
            "line-color": ["get", "color"],
            "line-opacity": ["coalesce", ["global-state", "opacity"], 1]
        }
    }));
    let errors = style
        .validate()
        .expect_err("unsupported expression contexts");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors
        .iter()
        .any(|error| error.to_string().contains("paint.line-width")));
    assert!(
        !errors
            .iter()
            .any(|error| error.to_string().contains("global-state")),
        "global-state evaluates to null when unset, so it is supported"
    );
}

#[test]
fn supported_symbol_expressions_and_literals_still_validate() {
    let style = style_with_layer(serde_json::json!({
        "id": "labels", "type": "symbol",
        "layout": {
            "text-field": "{name}", "text-font": ["Open Sans Regular"],
            "text-size": ["interpolate", ["linear"], ["zoom"], 0, 10, 10, 20],
            "text-allow-overlap": ["==", ["get", "kind"], "airport"],
            "text-offset": [0, 2], "symbol-height-offset": ["get", "height"],
            "symbol-height-anchor": "absolute"
        },
        "paint": {"text-color": "white", "text-halo-width": 1}
    }));
    assert_eq!(style.validate(), Ok(()));
    let encoded = serde_json::to_value(&style).expect("style serializes");
    assert_eq!(
        encoded["layers"][0]["layout"]["symbol-height-anchor"],
        "absolute"
    );
}

#[test]
fn symbol_values_that_are_not_evaluated_are_reported() {
    let style = style_with_layer(serde_json::json!({
        "id": "labels", "type": "symbol",
        "paint": {"text-color": ["get", "color"]},
        "layout": {"text-field": "label", "text-transform": ["get", "case"]}
    }));
    let errors = style.validate().expect_err("unsupported symbol values");
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(errors
        .iter()
        .any(|error| error.to_string().contains("paint.text-color")));
    assert!(errors
        .iter()
        .any(|error| error.to_string().contains("layout.text-transform")));
}

#[test]
fn overlap_modes_are_accepted_as_literals_only() {
    let accepted = style_with_layer(serde_json::json!({
        "id": "labels", "type": "symbol",
        "layout": {"text-field": "label", "text-overlap": "cooperative", "icon-overlap": "always"}
    }));
    accepted.validate().expect("overlap literals are supported");
    let rejected = style_with_layer(serde_json::json!({
        "id": "labels", "type": "symbol",
        "layout": {"text-field": "label", "text-overlap": ["step", ["zoom"], "never", 8, "always"],
            "icon-overlap": "sometimes"}
    }));
    let errors = rejected.validate().expect_err("unsupported overlap values");
    assert_eq!(errors.len(), 2, "{errors:?}");
}

#[test]
fn filter_and_paint_feature_contexts_are_distinguished() {
    let style = style_with_layer(serde_json::json!({
        "id": "roads", "type": "line", "filter": ["==", ["id"], 1],
        "paint": {"line-opacity": ["case", ["==", ["id"], 1], 1, 0]}
    }));
    let errors = style.validate().expect_err("paint has no ID context");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].to_string().contains("paint.line-opacity"));
}

#[test]
fn symbol_font_expressions_are_not_mistaken_for_literal_font_stacks() {
    let style = style_with_layer(serde_json::json!({
        "id": "labels", "type": "symbol", "layout": {"text-font": ["get", "fonts"]}
    }));
    let errors = style
        .validate()
        .expect_err("font expressions are not evaluated");
    assert!(errors[0].to_string().contains("layout.text-font"));
}

#[test]
fn elevation_is_only_available_to_color_relief() {
    let relief = style_with_layer(serde_json::json!({
        "id": "relief", "type": "color-relief", "paint": {
            "color-relief-color": ["interpolate", ["linear"], ["elevation"], 0, "black", 1000, "white"]
        }
    }));
    assert_eq!(relief.validate(), Ok(()));
    let fill = style_with_layer(serde_json::json!({
        "id": "land", "type": "fill", "paint": {"fill-opacity": ["elevation"]}
    }));
    let errors = fill
        .validate()
        .expect_err("fill evaluation has no elevation");
    assert!(errors[0].to_string().contains("paint.fill-opacity"));
}
