//! What a query finds is what placement shows: a label counts from the frame it is placed,
//! and neither a label that lost a collision nor one behind the globe can be found.
#![allow(clippy::expect_used, clippy::panic)]

use geozero::{mvt::Message, FeatureProcessor, GeomProcessor, PropertyProcessor};

use super::{atlas, fixture_map_after, SIZE};
use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    headless::map::{process_tile_layers, HeadlessMap, ProcessedLayers},
    sdf::{
        query::{QueryGeometry, QueryOptions},
        tessellation::TextTessellator,
    },
    style::{layer::LayerPaint, Style},
    vector::transferables::{DefaultSymbolLayerTessellated, SymbolLayerTessellated},
};

fn style(center: [f64; 2], zoom: f64, overlap: bool, globe: bool) -> Style {
    let mut style: serde_json::Value = serde_json::json!({
        "version": 8, "center": center, "zoom": zoom,
        "sources": {"map": {"type": "vector", "tiles": ["https://unused.invalid/{z}/{x}/{y}"],
            "maxzoom": 0}},
        "layers": [
            {"id": "background", "type": "background", "paint": {"background-color": "#334455"}},
            {"id": "point", "type": "circle", "source": "map", "source-layer": "places",
                "paint": {"circle-radius": 0.1}},
            {"id": "label", "type": "symbol", "source": "map", "source-layer": "places",
                "layout": {"text-field": "Alps", "text-size": 28, "text-allow-overlap": overlap},
                "paint": {"text-color": "#ff0000"}}
        ]
    });
    if globe {
        style["projection"] = serde_json::json!({"type": "globe"});
    }
    serde_json::from_value(style).expect("symbol style")
}

/// Labels with ids 1, 2, ... at `positions` in tile units of the world's single z0 tile.
fn labels(style: &Style, positions: &[[u32; 2]]) -> ProcessedLayers {
    labels_in(
        style,
        WorldTileCoords {
            x: 0,
            y: 0,
            z: ZoomLevel::from(0),
        },
        positions,
    )
}

/// Labels with ids 1, 2, ... at `positions` in tile units of the tile at `coords`.
fn labels_in(style: &Style, coords: WorldTileCoords, positions: &[[u32; 2]]) -> ProcessedLayers {
    coloured_labels_in(style, coords, positions, &[])
}

/// Labels as [`labels_in`], the `n`th with `colors[n]` as its `color` property when given.
fn coloured_labels_in(
    style: &Style,
    coords: WorldTileCoords,
    positions: &[[u32; 2]],
    colors: &[&str],
) -> ProcessedLayers {
    let source = geozero::mvt::tile::Layer {
        name: "places".into(),
        version: 2,
        extent: Some(4096),
        features: positions
            .iter()
            .enumerate()
            .map(|(index, [x, y])| geozero::mvt::tile::Feature {
                id: Some(index as u64 + 1),
                r#type: Some(1),
                geometry: vec![9, x << 1, y << 1],
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    let bytes = geozero::mvt::Tile {
        layers: vec![source.clone()],
    }
    .encode_to_vec();
    let mut layers =
        process_tile_layers(&bytes, &style.layers[1], coords, Default::default()).expect("points");
    let atlas = atlas();
    for style_layer in style.layers.iter().filter(|layer| layer.type_ == "symbol") {
        let Some(LayerPaint::Symbol(paint)) = &style_layer.paint else {
            panic!("symbol paint");
        };
        let mut layout = TextTessellator::default();
        layout.configure(paint.clone(), atlas.clone());
        layout.source_ids = source.features.iter().map(|feature| feature.id).collect();
        for (index, [x, y]) in positions.iter().enumerate() {
            if let Some(color) = colors.get(index) {
                layout
                    .property(0, "color", &geozero::ColumnValue::String(color))
                    .expect("property");
            }
            layout.point_begin(0).expect("point begin");
            layout
                .xy(f64::from(*x), f64::from(*y), 0)
                .expect("position");
            layout.point_end(0).expect("point end");
            layout.feature_end(index as u64).expect("feature");
        }
        layout.finish();
        layers
            .symbols
            .push(Box::new(DefaultSymbolLayerTessellated::build_from(
                coords,
                layout.quad_buffer.into(),
                layout.features,
                Some(atlas.clone()),
                source.clone(),
                style_layer.id.clone(),
            )));
    }
    layers
}

fn found(map: &HeadlessMap) -> Vec<Option<u64>> {
    found_in_layers(map).into_iter().map(|(_, id)| id).collect()
}

fn found_in_layers(map: &HeadlessMap) -> Vec<(String, Option<u64>)> {
    let screen = QueryGeometry::Box {
        min: [0.0, 0.0],
        max: [f64::from(SIZE), f64::from(SIZE)],
    };
    map.query_rendered_symbols_in(screen, &QueryOptions::default())
        .expect("query")
        .into_iter()
        .map(|symbol| (symbol.layer, symbol.id))
        .collect()
}

fn drawn(map: &HeadlessMap) -> usize {
    super::read_blocking(map)
        .chunks_exact(4)
        .filter(|pixel| pixel[0] > 180 && pixel[1] < 80 && pixel[2] < 80)
        .count()
}

#[tokio::test]
async fn a_label_is_found_from_the_frame_it_is_placed() {
    let style = style([0.0, 0.0], 2.0, false, false);
    let layers = labels(&style, &[[2048, 2048]]);
    let map = fixture_map_after(style, layers, 1, 1).await;
    assert_eq!(
        found(&map),
        [Some(1)],
        "a label placed this frame is queryable while it starts fading in, as in GL JS"
    );
}

#[tokio::test]
async fn of_two_colliding_labels_only_the_one_shown_is_found() {
    let positions = [[2048, 2048], [2050, 2049]];
    for (overlap, expected) in [(false, vec![Some(1)]), (true, vec![Some(2), Some(1)])] {
        let style = style([0.0, 0.0], 2.0, overlap, false);
        let layers = labels(&style, &positions);
        let map = fixture_map_after(style, layers, 1, 16).await;
        let mut ids = found(&map);
        ids.sort();
        let mut expected = expected.clone();
        expected.sort();
        assert_eq!(ids, expected, "text-allow-overlap {overlap}");
    }
}

#[tokio::test]
async fn a_label_behind_the_globe_is_neither_drawn_nor_found() {
    for (center, expected) in [([0.0, 0.0], vec![Some(1)]), ([180.0, 0.0], Vec::new())] {
        let style = style(center, 1.0, false, true);
        let layers = labels(&style, &[[2048, 2048]]);
        let map = fixture_map_after(style, layers, 1, 16).await;
        assert_eq!(found(&map), expected, "centre {center:?}");
        assert_eq!(
            drawn(&map) > 0,
            !expected.is_empty(),
            "the label is drawn exactly when it is found, centre {center:?}"
        );
    }
}

#[tokio::test]
async fn layers_alike_in_layout_share_one_placement_and_are_found_in_each() {
    let mut value = serde_json::to_value(style([0.0, 0.0], 2.0, false, false)).expect("style");
    let mut above = value["layers"][2].clone();
    above["id"] = "label-above".into();
    above["paint"] = serde_json::json!({"text-color": "#00ff00"});
    value["layers"].as_array_mut().expect("layers").push(above);
    let style: Style = serde_json::from_value(value).expect("style");
    let layers = labels(&style, &[[2048, 2048]]);
    let map = fixture_map_after(style, layers, 1, 16).await;
    assert_eq!(
        found_in_layers(&map),
        [
            ("label-above".to_owned(), Some(1)),
            ("label".to_owned(), Some(1))
        ],
        "as GL JS buckets them together, neither hides the other"
    );
}

#[tokio::test]
async fn a_label_held_by_a_parent_and_its_child_is_drawn_and_found_once() {
    // The north-west child of the world tile is loaded and its siblings are not, so the view
    // shows the child and, around it, the parent: both hold the label at the child's centre.
    let style = style([-90.0, 66.5], 1.0, true, false);
    let child = WorldTileCoords {
        x: 0,
        y: 0,
        z: ZoomLevel::from(1),
    };
    let mut layers = labels(&style, &[[1024, 1024]]);
    layers.append(&mut labels_in(&style, child, &[[2048, 2048]]));
    let both = fixture_map_after(style.clone(), layers, 1, 16).await;
    let alone = fixture_map_after(style.clone(), labels(&style, &[[1024, 1024]]), 1, 16).await;
    assert_eq!(found(&both), [Some(1)], "one label is one hit");
    assert_eq!(found(&alone), [Some(1)]);
    let (drawn_both, drawn_alone) = (drawn(&both), drawn(&alone));
    assert!(drawn_alone > 0);
    assert!(
        drawn_both <= drawn_alone + drawn_alone / 4,
        "the label is drawn once: {drawn_both} red pixels against {drawn_alone} alone"
    );
}

fn coloured(map: &HeadlessMap) -> [usize; 2] {
    let pixels = super::read_blocking(map);
    let count = |channel: usize| {
        pixels
            .chunks_exact(4)
            .filter(|pixel| {
                pixel[channel] > 180 && (0..3).all(|other| other == channel || pixel[other] < 80)
            })
            .count()
    };
    [count(0), count(1)]
}

#[tokio::test]
async fn overlapping_labels_are_drawn_and_found_in_the_rotated_order() {
    let mut on_top = Vec::new();
    for bearing in [0.0, 180.0] {
        let mut value = serde_json::to_value(style([0.0, 0.0], 2.0, true, false)).expect("style");
        value["bearing"] = bearing.into();
        value["layers"][2]["paint"]["text-color"] = serde_json::json!(["get", "color"]);
        let style: Style = serde_json::from_value(value).expect("style");
        let world = WorldTileCoords {
            x: 0,
            y: 0,
            z: ZoomLevel::from(0),
        };
        // The green label sits a little lower on the map, so it is the lower one unturned.
        let layers = coloured_labels_in(
            &style,
            world,
            &[[2048, 2048], [2052, 2051]],
            &["#ff0000", "#00ff00"],
        );
        let map = fixture_map_after(style, layers, 1, 16).await;
        let [red, green] = coloured(&map);
        assert!(
            red > 0 && green > 0,
            "both labels show at {bearing}: {red} {green}"
        );
        let shown = if green > red { Some(2) } else { Some(1) };
        assert_eq!(
            found(&map).first().copied().flatten(),
            shown,
            "the label drawn on top is found first at {bearing}: {red} red, {green} green"
        );
        on_top.push(shown);
    }
    assert_eq!(
        on_top,
        [Some(2), Some(1)],
        "turning the map round puts the other label on top"
    );
}
