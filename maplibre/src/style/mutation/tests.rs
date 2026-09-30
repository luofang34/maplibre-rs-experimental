#![allow(clippy::expect_used, clippy::panic)]
use csscolorparser::Color;
use serde_json::{json, Value};

use super::StyleMutationError;
use crate::style::{layer::LayerPaint, source::Source, Style};

fn style() -> Style {
    let mut style: Style = serde_json::from_value(json!({"version": 8,
        "state": {"tint": {"default": "#ff0000"}},
        "sources": {
            "shapes": {"type": "geojson", "data": {"type": "Point", "coordinates": [0, 0]}},
            "roads": {"type": "vector", "tiles": ["t/{z}/{x}/{y}"]}},
        "layers": [
            {"id": "paper", "type": "background", "paint": {"background-color": "#ffffff"}},
            {"id": "land", "type": "fill", "source": "shapes", "paint": {"fill-color": "#00ff00"}},
            {"id": "tinted", "type": "fill", "source": "shapes",
             "paint": {"fill-color": ["global-state", "tint"]}}]}))
    .expect("style");
    style.resolve_global_state();
    style
}

fn ids(style: &Style) -> Vec<String> {
    style.layers.iter().map(|layer| layer.id.clone()).collect()
}

fn indexes(style: &Style) -> Vec<u32> {
    style.layers.iter().map(|layer| layer.index).collect()
}

fn fill(style: &Style, id: &str) -> Option<Color> {
    let layer = style.layers.iter().find(|layer| layer.id == id)?;
    let Some(LayerPaint::Fill(paint)) = &layer.paint else {
        panic!("fill layer");
    };
    paint.fill_color.as_ref()?.evaluate_at_zoom(0.0)
}

fn road() -> Value {
    json!({"id": "road", "type": "line", "source": "roads", "source-layer": "roads",
        "paint": {"line-color": "#333333"}})
}

#[test]
fn layers_are_added_removed_and_moved_with_their_order_renumbered() {
    let mut style = style();
    let change = style
        .add_layer(road(), Some("land"))
        .expect("add below land");
    assert_eq!(ids(&style), ["paper", "road", "land", "tinted"]);
    assert_eq!(indexes(&style), [0, 1, 2, 3]);
    assert!(change.redraw_tiles);
    assert!(change.layers.contains(&"road".to_owned()));
    assert!(
        change.layers.contains(&"land".to_owned()),
        "land moved up one place"
    );
    assert!(
        !change.layers.contains(&"paper".to_owned()),
        "paper did not move"
    );

    let change = style.move_layer("road", None).expect("move to the top");
    assert_eq!(ids(&style), ["paper", "land", "tinted", "road"]);
    assert_eq!(indexes(&style), [0, 1, 2, 3]);
    assert!(change.redraw_tiles);

    let change = style.remove_layer("land").expect("remove");
    assert_eq!(ids(&style), ["paper", "tinted", "road"]);
    assert_eq!(indexes(&style), [0, 1, 2]);
    assert!(change.redraw_tiles && change.layers.contains(&"land".to_owned()));

    style
        .add_layer(
            json!({"id": "land", "type": "fill", "source": "shapes",
        "paint": {"fill-color": "#0000ff"}}),
            None,
        )
        .expect("the id can be added again");
    assert_eq!(ids(&style), ["paper", "tinted", "road", "land"]);
}

#[test]
fn invalid_layers_and_positions_are_refused_without_changing_anything() {
    let mut style = style();
    let before = ids(&style);
    assert!(matches!(
        style.add_layer(
            json!({"id": "land", "type": "fill", "source": "shapes"}),
            None
        ),
        Err(StyleMutationError::DuplicateLayer { .. })
    ));
    assert!(matches!(
        style.add_layer(
            json!({"id": "x", "type": "fill", "source": "nowhere"}),
            None
        ),
        Err(StyleMutationError::MissingSource { .. })
    ));
    assert!(matches!(
        style.add_layer(road(), Some("nowhere")),
        Err(StyleMutationError::UnknownLayer { .. })
    ));
    assert!(matches!(
        style.add_layer(json!({"id": "x", "type": "nonsense"}), None),
        Err(StyleMutationError::Unsupported { .. })
    ));
    assert!(matches!(
        style.add_layer(
            json!({"id": "x", "type": "fill", "source": "shapes",
            "paint": {"fill-color": ["not-an-operator"]}}),
            None
        ),
        Err(StyleMutationError::Unsupported { .. })
    ));
    assert!(matches!(
        style.move_layer("land", Some("nowhere")),
        Err(StyleMutationError::UnknownLayer { .. })
    ));
    assert!(matches!(
        style.remove_layer("nowhere"),
        Err(StyleMutationError::UnknownLayer { .. })
    ));
    assert_eq!(ids(&style), before);
    assert_eq!(indexes(&style), [0, 1, 2]);
}

#[test]
fn properties_filters_and_zoom_ranges_are_changed_and_invalid_ones_refused() {
    let mut style = style();
    let change = style
        .set_paint_property("land", "fill-color", json!("#123456"))
        .expect("set paint");
    assert_eq!(
        fill(&style, "land"),
        Some(Color::from_rgba8(0x12, 0x34, 0x56, 255))
    );
    assert_eq!(change.layers, ["land"]);
    assert!(change.redraw_tiles);

    assert!(
        style
            .set_paint_property("land", "fill-color", json!("#123456"))
            .expect("same value")
            .layers
            .is_empty(),
        "setting the same value changes nothing"
    );

    style
        .set_paint_property("land", "fill-color", Value::Null)
        .expect("reset");
    assert_eq!(fill(&style, "land"), None, "null restores the default");

    assert!(matches!(
        style.set_paint_property("land", "fill-color", json!(["not-an-operator"])),
        Err(StyleMutationError::Unsupported { .. })
    ));
    assert!(
        matches!(
            style.set_paint_property("land", "fill-glitter", json!(1)),
            Err(StyleMutationError::Unsupported { .. })
        ),
        "a property this renderer does not have is refused"
    );
    assert!(matches!(
        style.set_paint_property("nowhere", "fill-color", json!("red")),
        Err(StyleMutationError::UnknownLayer { .. })
    ));

    style
        .set_filter("land", Some(json!(["==", "kind", "park"])))
        .expect("filter");
    assert_eq!(style.layers[1].filter, Some(json!(["==", "kind", "park"])));
    assert!(matches!(
        style.set_filter("land", Some(json!(["within", {}]))),
        Err(StyleMutationError::Unsupported { .. })
    ));
    style.set_filter("land", None).expect("remove filter");
    assert_eq!(style.layers[1].filter, None);

    style
        .set_layer_zoom_range("land", Some(3.0), Some(9.0))
        .expect("range");
    assert_eq!(
        (style.layers[1].minzoom, style.layers[1].maxzoom),
        (Some(3.0), Some(9.0))
    );
    style
        .set_layout_property("land", "visibility", json!("none"))
        .expect("hide");
    assert!(style.layers[1].is_hidden());
    style
        .set_layout_property("land", "visibility", Value::Null)
        .expect("show");
    assert!(!style.layers[1].is_hidden());
}

#[test]
fn background_edits_need_no_tile_refresh() {
    let mut style = style();
    let change = style
        .set_paint_property("paper", "background-color", json!("#000000"))
        .expect("set background");
    assert_eq!(change.layers, ["paper"]);
    assert!(
        !change.redraw_tiles,
        "a background is drawn from the style each frame"
    );
}

#[test]
fn editing_a_layer_that_reads_global_state_keeps_it_reading_state() {
    let mut style = style();
    style
        .set_layer_zoom_range("tinted", Some(2.0), None)
        .expect("edit an unrelated field");
    assert_eq!(style.layers[2].minzoom, Some(2.0));
    let changed = style.set_global_state("tint", json!("#00ffff"));
    assert_eq!(changed, ["tinted"], "the dependency survived the edit");
    assert_eq!(
        fill(&style, "tinted"),
        Some(Color::from_rgba8(0, 255, 255, 255))
    );
    style
        .set_paint_property("tinted", "fill-opacity", json!(0.5))
        .expect("edit another property");
    style.set_global_state("tint", json!("#ffff00"));
    assert_eq!(
        fill(&style, "tinted"),
        Some(Color::from_rgba8(255, 255, 0, 255))
    );
}

#[test]
fn a_layer_added_with_a_state_reference_reads_the_current_value() {
    let mut style = style();
    style.set_global_state("tint", json!("#00ffff"));
    style
        .add_layer(
            json!({"id": "more", "type": "fill", "source": "shapes",
            "paint": {"fill-color": ["global-state", "tint"]}}),
            None,
        )
        .expect("add");
    assert_eq!(
        fill(&style, "more"),
        Some(Color::from_rgba8(0, 255, 255, 255))
    );
}

#[test]
fn sources_are_added_removed_and_a_readded_geojson_source_gets_a_fresh_generation() {
    let mut style = style();
    assert!(matches!(
        style.remove_source("shapes"),
        Err(StyleMutationError::SourceInUse { .. })
    ));
    assert!(matches!(
        style.remove_source("nowhere"),
        Err(StyleMutationError::UnknownSource { .. })
    ));
    let Some(Source::GeoJson(original)) = style.sources.get("shapes").cloned() else {
        panic!("geojson source");
    };
    let first_generation = original.generation;
    style.remove_layer("land").expect("remove layer");
    style.remove_layer("tinted").expect("remove layer");
    style.remove_source("shapes").expect("unused now");
    assert!(!style.sources.contains_key("shapes"));
    assert!(matches!(
        style.add_source("roads", Source::GeoJson(original.clone())),
        Err(StyleMutationError::DuplicateSource { .. })
    ));
    style
        .add_source("shapes", Source::GeoJson(original))
        .expect("add again");
    let Some(Source::GeoJson(readded)) = style.sources.get("shapes") else {
        panic!("geojson source");
    };
    assert_ne!(
        readded.generation, first_generation,
        "workers cannot serve the old data"
    );
}
