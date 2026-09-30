#![allow(clippy::expect_used, clippy::panic)]
use csscolorparser::Color;
use serde_json::{json, Value};

use crate::style::{
    filter::{FeatureContext, Filter, GeometryType},
    layer::LayerPaint,
    Style,
};

fn style() -> Style {
    let mut style: Style = serde_json::from_value(json!({"version": 8,
        "state": {"color": {"default": "magenta"}, "minRank": {"default": 5}},
        "sources": {},
        "layers": [
            {"id": "tinted", "type": "fill", "paint": {"fill-color": ["global-state", "color"]}},
            {"id": "plain", "type": "fill", "paint": {"fill-color": "#00ff00"}},
            {"id": "ranked", "type": "line", "paint": {"line-color": "#ff0000"},
             "filter": [">=", ["get", "rank"], ["global-state", "minRank"]]}]}))
    .expect("style");
    style.resolve_global_state();
    style
}

fn fill_color(style: &Style, id: &str) -> Option<Color> {
    let layer = style.layers.iter().find(|layer| layer.id == id)?;
    let Some(LayerPaint::Fill(paint)) = &layer.paint else {
        panic!("fill paint");
    };
    paint.fill_color.as_ref()?.evaluate_at_zoom(0.0)
}

fn passes(style: &Style, rank: f64) -> bool {
    let layer = style
        .layers
        .iter()
        .find(|layer| layer.id == "ranked")
        .expect("layer");
    let filter = Filter::parse(layer.filter.as_ref().expect("filter")).expect("filter parses");
    let properties = [(
        "rank".to_owned(),
        crate::style::expression::Value::Number(rank),
    )]
    .into_iter()
    .collect();
    filter.evaluate(&FeatureContext {
        properties: &properties,
        geometry_type: GeometryType::Point,
        id: None,
        zoom: 0.0,
    })
}

#[test]
fn declared_defaults_apply_when_the_style_is_loaded() {
    let style = style();
    assert_eq!(
        fill_color(&style, "tinted"),
        Some(Color::from_rgba8(255, 0, 255, 255))
    );
    assert_eq!(
        fill_color(&style, "plain"),
        Some(Color::from_rgba8(0, 255, 0, 255))
    );
    assert!(
        passes(&style, 5.0) && !passes(&style, 4.0),
        "the default rank threshold is 5"
    );
}

#[test]
fn a_set_value_overrides_the_default_and_null_restores_it() {
    let mut style = style();
    let changed = style.set_global_state("color", json!("cyan"));
    assert_eq!(changed, ["tinted"]);
    assert_eq!(
        fill_color(&style, "tinted"),
        Some(Color::from_rgba8(0, 255, 255, 255))
    );
    let changed = style.set_global_state("color", json!("yellow"));
    assert_eq!(
        changed,
        ["tinted"],
        "a second change starts from the declared layer"
    );
    assert_eq!(
        fill_color(&style, "tinted"),
        Some(Color::from_rgba8(255, 255, 0, 255))
    );
    let changed = style.set_global_state("color", Value::Null);
    assert_eq!(changed, ["tinted"]);
    assert_eq!(
        fill_color(&style, "tinted"),
        Some(Color::from_rgba8(255, 0, 255, 255))
    );
}

#[test]
fn only_layers_that_read_the_key_change() {
    let mut style = style();
    assert_eq!(style.set_global_state("minRank", json!(8)), ["ranked"]);
    assert!(
        passes(&style, 9.0) && !passes(&style, 7.0),
        "the filter follows the new threshold"
    );
    assert!(
        style.set_global_state("color", json!("magenta")).is_empty(),
        "same value as the default"
    );
    assert!(
        style.set_global_state("unused", json!(1)).is_empty(),
        "nothing reads this key"
    );
    assert!(
        style.set_global_state("minRank", json!(8)).is_empty(),
        "setting it again changes nothing"
    );
}

#[test]
fn a_key_without_value_or_default_reads_as_null() {
    let mut style: Style = serde_json::from_value(json!({"version": 8, "sources": {},
        "layers": [{"id": "ranked", "type": "line", "paint": {"line-color": "#ff0000"},
            "filter": ["==", ["global-state", "flag"], true]}]}))
    .expect("style");
    style.resolve_global_state();
    assert_eq!(style.global_state_value("flag"), Value::Null);
    let filter = Filter::parse(style.layers[0].filter.as_ref().expect("filter")).expect("parses");
    let properties = Default::default();
    let feature = FeatureContext {
        properties: &properties,
        geometry_type: GeometryType::Point,
        id: None,
        zoom: 0.0,
    };
    assert!(!filter.evaluate(&feature), "null is not true");
    style.set_global_state("flag", json!(true));
    let filter = Filter::parse(style.layers[0].filter.as_ref().expect("filter")).expect("parses");
    assert!(filter.evaluate(&feature));
}

#[test]
fn global_state_lists_declared_and_set_keys() {
    let mut style = style();
    style.set_global_state("extra", json!([1, 2]));
    let values = style.global_state_values();
    assert_eq!(values["color"], "magenta");
    assert_eq!(values["minRank"], 5);
    assert_eq!(values["extra"], json!([1, 2]));
}

#[test]
fn resolved_layers_keep_their_position_and_survive_serialization() {
    let mut style = style();
    style.set_global_state("color", json!("cyan"));
    assert_eq!(
        style
            .layers
            .iter()
            .map(|layer| layer.index)
            .collect::<Vec<_>>(),
        [0, 1, 2]
    );
    let sent: Style = serde_json::from_value(serde_json::to_value(&style).expect("serializes"))
        .expect("deserializes");
    assert_eq!(
        fill_color(&sent, "tinted"),
        Some(Color::from_rgba8(0, 255, 255, 255))
    );
    assert_eq!(
        sent.global_state["color"], "cyan",
        "workers receive the values too"
    );
}

#[test]
fn array_values_are_wrapped_so_they_stay_data() {
    let mut style: Style = serde_json::from_value(json!({"version": 8, "sources": {},
        "state": {"kinds": {"default": ["a", "b"]}},
        "layers": [{"id": "ranked", "type": "line", "paint": {"line-color": "#ff0000"},
            "filter": ["in", ["get", "kind"], ["global-state", "kinds"]]}]}))
    .expect("style");
    style.resolve_global_state();
    assert_eq!(
        style.layers[0].filter,
        Some(json!(["in", ["get", "kind"], ["literal", ["a", "b"]]]))
    );
}

fn ids_and_types(style: &Style) -> Vec<(String, String)> {
    style
        .layers
        .iter()
        .map(|layer| (layer.id.clone(), layer.type_.clone()))
        .collect()
}

#[test]
fn layers_that_share_an_id_are_left_alone_instead_of_clobbering_each_other() {
    let mut style: Style = serde_json::from_value(json!({"version": 8, "sources": {},
        "layers": [
            {"id": "twin", "type": "line", "paint": {"line-color": ["global-state", "c"]}},
            {"id": "twin", "type": "fill", "paint": {"fill-color": "#0000ff"}}]}))
    .expect("style");
    style.resolve_global_state();
    assert!(style.set_global_state("c", json!("green")).is_empty());
    assert_eq!(
        ids_and_types(&style),
        [
            ("twin".into(), "line".into()),
            ("twin".into(), "fill".into())
        ],
        "neither layer changed type"
    );
}

#[test]
fn a_replaced_or_edited_layer_is_not_resurrected_from_its_old_template() {
    let mut style = style();
    style.set_global_state("color", json!("cyan"));
    let replacement: crate::style::layer::StyleLayer = serde_json::from_value(
        json!({"id": "tinted", "type": "fill", "paint": {"fill-color": "#123456"}}),
    )
    .expect("layer");
    let index = style.layers[0].index;
    style.layers[0] = replacement;
    style.layers[0].index = index;
    assert!(style.set_global_state("color", json!("yellow")).is_empty());
    assert_eq!(
        fill_color(&style, "tinted"),
        Some(Color::from_rgba8(0x12, 0x34, 0x56, 255))
    );
    assert!(
        !style.state_templates.contains_key("tinted"),
        "a layer that no longer reads state is released"
    );
}

#[test]
fn a_layer_that_reads_state_again_after_an_edit_is_tracked_from_the_new_declaration() {
    let mut style = style();
    let edited: crate::style::layer::StyleLayer = serde_json::from_value(json!({"id": "plain",
        "type": "fill", "paint": {"fill-color": ["global-state", "color"]}}))
    .expect("layer");
    let index = style.layers[1].index;
    style.layers[1] = edited;
    style.layers[1].index = index;
    assert_eq!(style.resolve_global_state(), ["plain"]);
    assert_eq!(
        fill_color(&style, "plain"),
        Some(Color::from_rgba8(255, 0, 255, 255))
    );
    assert_eq!(
        style.set_global_state("color", json!("cyan")),
        ["tinted", "plain"]
    );
}

#[test]
fn templates_of_removed_layers_are_dropped() {
    let mut style = style();
    assert!(style.state_templates.contains_key("tinted"));
    style.layers.retain(|layer| layer.id != "tinted");
    style.resolve_global_state();
    assert!(!style.state_templates.contains_key("tinted"));
}

#[test]
fn a_value_of_the_wrong_type_falls_back_to_the_layers_default() {
    let mut style = style();
    style.set_global_state("color", json!(5));
    assert_eq!(
        fill_color(&style, "tinted"),
        None,
        "a number is not a color"
    );
    style.set_global_state("color", json!("cyan"));
    assert_eq!(
        fill_color(&style, "tinted"),
        Some(Color::from_rgba8(0, 255, 255, 255))
    );
}
