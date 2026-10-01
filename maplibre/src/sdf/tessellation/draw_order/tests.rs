#![allow(clippy::expect_used, clippy::panic)]
use super::*;

fn paint(properties: serde_json::Value) -> SymbolPaint {
    SymbolPaint {
        properties: properties.as_object().expect("an object").clone(),
        ..SymbolPaint::default()
    }
}

#[test]
fn labels_that_may_overlap_draw_by_height_unless_a_sort_key_orders_them() {
    assert!(sorts_by_height(
        &paint(serde_json::json!({"icon-allow-overlap": true})),
        0.0
    ));
    assert!(sorts_by_height(
        &paint(serde_json::json!({"icon-ignore-placement": true, "symbol-z-order": "viewport-y"})),
        0.0
    ));
    assert!(!sorts_by_height(
        &paint(serde_json::json!({"icon-allow-overlap": true, "symbol-sort-key": 1})),
        0.0
    ));
    assert!(!sorts_by_height(
        &paint(serde_json::json!({"icon-allow-overlap": true, "symbol-z-order": "source"})),
        0.0
    ));
}

#[test]
fn labels_that_never_overlap_keep_their_order() {
    assert!(!sorts_by_height(&paint(serde_json::json!({})), 0.0));
    assert!(!sorts_by_height(
        &paint(serde_json::json!({"text-overlap": "never"})),
        0.0
    ));
}
