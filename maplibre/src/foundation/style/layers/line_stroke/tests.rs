#![allow(clippy::expect_used, clippy::panic)]
use super::*;

#[test]
fn the_stroke_reads_cap_join_and_miter_limit_from_the_layout() {
    let layer: StyleLayer = serde_json::from_value(serde_json::json!({
        "id": "roads", "type": "line", "source": "s",
        "layout": {
            "line-cap": "round",
            "line-join": "bevel",
            "line-miter-limit": 5,
            "line-round-limit": 1.6
        }
    }))
    .expect("layer");
    assert_eq!(
        LineStroke::of_layer(&layer),
        LineStroke {
            cap: LineCap::Round,
            join: LineJoin::Bevel,
            miter_limit: 5.0,
            round_limit: 1.6,
        }
    );
}

#[test]
fn a_layer_without_them_has_the_defaults() {
    let layer: StyleLayer = serde_json::from_value(serde_json::json!({
        "id": "roads", "type": "line", "source": "s"
    }))
    .expect("layer");
    assert_eq!(LineStroke::of_layer(&layer), LineStroke::default());
    assert!((LineStroke::default().round_limit - 1.05).abs() < 1e-6);
}

#[test]
fn line_round_limit_supports_zoom_expressions_and_retessellation_mutation() {
    let layer: StyleLayer = serde_json::from_value(serde_json::json!({
        "id": "roads",
        "type": "line",
        "source": "s",
        "layout": {
            "line-join": "round",
            "line-round-limit": ["interpolate", ["linear"], ["zoom"], 10, 1.05, 14, 2.05]
        }
    }))
    .expect("layer");
    assert!((LineStroke::of_layer_at_zoom(&layer, 10.0).round_limit - 1.05).abs() < 1e-5);
    assert!((LineStroke::of_layer_at_zoom(&layer, 12.0).round_limit - 1.55).abs() < 1e-5);
    assert!((LineStroke::of_layer_at_zoom(&layer, 14.0).round_limit - 2.05).abs() < 1e-5);

    let mut style: crate::style::Style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "sources": { "s": { "type": "geojson", "data": { "type": "FeatureCollection", "features": [] } } },
        "layers": [{ "id": "roads", "type": "line", "source": "s", "layout": { "line-join": "round" } }]
    }))
    .expect("style");
    let change = style
        .set_layout_property("roads", "line-round-limit", serde_json::json!(1.8))
        .expect("mutates");
    assert_eq!(change.layers, vec!["roads".to_string()]);
    assert!(
        change.redraw_tiles,
        "line-round-limit layout change must redraw tiles"
    );
}

#[test]
fn unknown_values_are_not_accepted() {
    assert!(LineStroke::accepts(
        "line-join",
        &serde_json::json!("round")
    ));
    assert!(LineStroke::accepts(
        "line-round-limit",
        &serde_json::json!(1.5)
    ));
    assert!(!LineStroke::accepts(
        "line-round-limit",
        &serde_json::json!(["get", "r"])
    ));
    assert!(!LineStroke::accepts(
        "line-join",
        &serde_json::json!("sharp")
    ));
    assert!(!LineStroke::accepts(
        "line-cap",
        &serde_json::json!(["get", "cap"])
    ));
}
