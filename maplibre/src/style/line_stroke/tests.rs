#![allow(clippy::expect_used, clippy::panic)]
use super::*;

#[test]
fn the_stroke_reads_cap_join_and_miter_limit_from_the_layout() {
    let layer: StyleLayer = serde_json::from_value(serde_json::json!({
        "id": "roads", "type": "line", "source": "s",
        "layout": {"line-cap": "round", "line-join": "bevel", "line-miter-limit": 5}
    }))
    .expect("layer");
    assert_eq!(
        LineStroke::of_layer(&layer),
        LineStroke {
            cap: LineCap::Round,
            join: LineJoin::Bevel,
            miter_limit: 5.0
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
}

#[test]
fn unknown_values_are_not_accepted() {
    assert!(LineStroke::accepts(
        "line-join",
        &serde_json::json!("round")
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
