use serde_json::json;

use crate::style::{source::Source, Style};

fn base_style() -> Style {
    serde_json::from_value(json!({
        "version": 8,
        "sources": {
            "roads": {
                "type": "vector",
                "tiles": ["https://tiles.example/{z}/{x}/{y}.pbf"]
            },
            "parks": {
                "type": "geojson",
                "data": {
                    "type": "FeatureCollection",
                    "features": [{
                        "type": "Feature",
                        "properties": {},
                        "geometry": { "type": "Point", "coordinates": [0.0, 0.0] }
                    }]
                }
            },
            "overlay": {
                "type": "image",
                "url": "https://img.example/overlay.png",
                "coordinates": [[-1.0, 1.0], [1.0, 1.0], [1.0, -1.0], [-1.0, -1.0]]
            }
        },
        "layers": [
            {
                "id": "bg",
                "type": "background",
                "paint": { "background-color": "#112233" }
            },
            {
                "id": "park-fill",
                "type": "fill",
                "source": "parks",
                "paint": { "fill-color": "#00aa00" }
            },
            {
                "id": "road-line",
                "type": "line",
                "source": "roads",
                "source-layer": "transportation",
                "paint": { "line-color": "#ffffff", "line-width": 2.0 }
            }
        ]
    }))
    .expect("valid base style")
}

fn generation_of(style: &Style, name: &str) -> u64 {
    match style.sources.get(name) {
        Some(Source::GeoJson(g)) => g.generation,
        Some(Source::Image(i)) => i.generation,
        other => panic!("unexpected source {other:?}"),
    }
}

#[test]
fn identical_sources_with_paint_only_edit_preserve_generations_without_tile_redraw() {
    let mut current = base_style();
    let parks_gen = generation_of(&current, "parks");
    let overlay_gen = generation_of(&current, "overlay");

    // Re-parse from scratch so incoming GeoJson/Image start with brand-new random generations.
    let mut next = base_style();
    assert_ne!(generation_of(&next, "parks"), parks_gen);
    next.set_paint_property("park-fill", "fill-color", json!("#00ff00"))
        .expect("edit paint");

    let diff = current.diff_and_replace(next);
    assert!(
        !diff.redraw_tiles,
        "paint-only change must not refetch vector tiles"
    );
    assert!(diff.added_sources.is_empty());
    assert!(diff.removed_sources.is_empty());
    assert!(diff.updated_sources.is_empty());
    assert_eq!(diff.updated_layers, vec!["park-fill"]);
    assert_eq!(
        generation_of(&current, "parks"),
        parks_gen,
        "unchanged GeoJSON source preserves its generation"
    );
    assert_eq!(
        generation_of(&current, "overlay"),
        overlay_gen,
        "unchanged Image source preserves its generation"
    );
}

#[test]
fn adding_removing_or_modifying_single_source_or_layer_marks_only_affected_items() {
    let mut current = base_style();
    let parks_gen = generation_of(&current, "parks");

    let mut next: serde_json::Value = serde_json::to_value(&current).expect("serialize");
    // Modify `overlay` coordinates, remove `roads` + `road-line`, add `labels-bg`, leave `parks` intact.
    next["sources"]["overlay"]["coordinates"] =
        json!([[-2.0, 2.0], [2.0, 2.0], [2.0, -2.0], [-2.0, -2.0]]);
    next["sources"]
        .as_object_mut()
        .expect("sources")
        .remove("roads");
    next["sources"]["extra"] = json!({
        "type": "geojson",
        "data": { "type": "FeatureCollection", "features": [] }
    });
    let layers = next["layers"].as_array_mut().expect("layers");
    layers.retain(|l| l["id"] != "road-line");
    layers.push(json!({
        "id": "top-bg",
        "type": "background",
        "paint": { "background-color": "#000000" }
    }));

    let parsed_next: Style = serde_json::from_value(next).expect("deserialize next");
    let diff = current.diff_and_replace(parsed_next);

    assert_eq!(diff.added_sources, vec!["extra"]);
    assert_eq!(diff.removed_sources, vec!["roads"]);
    assert_eq!(diff.updated_sources, vec!["overlay"]);
    assert_eq!(diff.reloaded_image_sources, vec!["overlay"]);
    assert_eq!(diff.added_layers, vec!["top-bg"]);
    assert_eq!(diff.removed_layers, vec!["road-line"]);
    assert!(diff.updated_layers.is_empty());
    assert!(!diff.redraw_tiles);
    assert_eq!(
        generation_of(&current, "parks"),
        parks_gen,
        "unmodified sibling source keeps its generation"
    );
}

#[test]
fn root_terrain_projection_and_light_changes_are_detected_and_transitions_honored() {
    let mut current = base_style();
    let mut doc: serde_json::Value = serde_json::to_value(&current).expect("serialize");
    doc["sources"]["dem"] = json!({
        "type": "raster-dem",
        "tiles": ["https://dem.example/{z}/{x}/{y}.png"]
    });
    doc["terrain"] = json!({ "source": "dem", "exaggeration": 1.5 });
    doc["projection"] = json!({ "type": "globe" });
    doc["light"] = json!({ "anchor": "map", "intensity": 0.8 });
    doc["transition"] = json!({ "duration": 400, "delay": 0 });
    doc["layers"][0]["paint"]["background-color"] = json!("#ffffff");

    let next: Style = serde_json::from_value(doc).expect("deserialize next");
    let diff = current.diff_and_replace(next);

    assert!(diff.terrain_changed);
    assert!(diff.projection_changed);
    assert!(diff.light_changed);
    assert!(diff.transition_changed);
    assert!(
        diff.redraw_tiles,
        "globe projection switch re-subdivides vector tiles"
    );
    assert!(
        current.has_active_transitions(),
        "paint change with active root transition starts an animated transition"
    );
    current.settle_transitions_at(400.0);
    assert!(!current.has_active_transitions());
}
