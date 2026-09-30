#![allow(clippy::expect_used, clippy::panic)]
use super::*;

fn fill(pattern: serde_json::Value) -> LayerPaint {
    let layer: crate::style::layer::StyleLayer = serde_json::from_value(serde_json::json!({
        "id": "land", "type": "fill", "paint": {"fill-pattern": pattern}
    }))
    .expect("layer");
    layer.paint.expect("fill paint")
}

#[test]
fn a_literal_names_its_image_at_every_zoom() {
    assert_eq!(
        pattern_name(&fill(serde_json::json!("dots")), 3.0).as_deref(),
        Some("dots")
    );
}

#[test]
fn a_zoom_expression_picks_the_image_of_the_zoom() {
    let paint = fill(serde_json::json!(["step", ["zoom"], "small", 10, "large"]));
    assert_eq!(pattern_name(&paint, 4.0).as_deref(), Some("small"));
    assert_eq!(pattern_name(&paint, 12.0).as_deref(), Some("large"));
}

#[test]
fn a_fill_without_a_pattern_names_none() {
    let layer: crate::style::layer::StyleLayer = serde_json::from_value(serde_json::json!({
        "id": "land", "type": "fill", "paint": {"fill-color": "red"}
    }))
    .expect("layer");
    assert_eq!(pattern_name(&layer.paint.expect("paint"), 3.0), None);
}
