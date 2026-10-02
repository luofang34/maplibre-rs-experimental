use super::*;

fn style(layers: serde_json::Value) -> Style {
    serde_json::from_value(serde_json::json!({"version": 8, "sources": {}, "layers": layers}))
        .expect("style")
}

fn symbol(id: &str, filter: Option<serde_json::Value>, color: &str) -> serde_json::Value {
    let mut layer = serde_json::json!({"id": id, "type": "symbol", "source": "s",
        "source-layer": "places", "layout": {"text-field": "{name}"},
        "paint": {"text-color": color}});
    if let Some(filter) = filter {
        layer["filter"] = filter;
    }
    layer
}

#[test]
fn layers_alike_but_for_paint_share_the_lowest_one_s_placement() {
    let leaders = leaders(&style(serde_json::json!([
        symbol("low", None, "#f00"),
        symbol("high", None, "#0f0"),
    ])));
    assert_eq!(leaders.get("high").map(String::as_str), Some("low"));
    assert_eq!(leaders.get("low").map(String::as_str), Some("low"));
}

#[test]
fn a_filter_or_a_layout_property_keeps_layers_apart() {
    let leaders = leaders(&style(serde_json::json!([
        symbol("all", None, "#f00"),
        symbol("towns", Some(serde_json::json!(["==", "kind", "town"])), "#f00"),
        {"id": "bigger", "type": "symbol", "source": "s", "source-layer": "places",
            "layout": {"text-field": "{name}", "text-size": 20}},
    ])));
    assert!(leaders.is_empty(), "no two layers group: {leaders:?}");
}
