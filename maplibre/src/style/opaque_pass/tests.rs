use serde_json::json;

use super::*;

fn style(layers: serde_json::Value) -> Style {
    serde_json::from_value(json!({"version": 8, "sources": {}, "layers": layers}))
        .expect("style parses")
}

#[test]
fn a_heatmap_lands_over_the_opaque_layers_above_it() {
    let style = style(json!([
        {"id": "heat", "type": "heatmap", "source": "s"},
        {"id": "see-through", "type": "background", "paint": {"background-color": "rgba(0,0,255,0.5)"}},
        {"id": "ground", "type": "background", "paint": {"background-color": "blue"}},
    ]));
    assert_eq!(style.opaque_layer_above(&style.layers[0], 0.0), Some(2));
    assert_eq!(style.opaque_layer_above(&style.layers[2], 0.0), None);
}

#[test]
fn a_fill_extrusion_below_the_opaque_layer_ends_the_opaque_pass() {
    let style = style(json!([
        {"id": "heat", "type": "heatmap", "source": "s"},
        {"id": "tower", "type": "fill-extrusion", "source": "s"},
        {"id": "ground", "type": "background", "paint": {"background-color": "blue"}},
    ]));
    assert_eq!(style.opaque_layer_above(&style.layers[0], 0.0), None);
}
