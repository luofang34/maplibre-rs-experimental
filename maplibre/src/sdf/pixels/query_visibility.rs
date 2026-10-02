//! What a query finds is what placement shows: a label counts from the frame it is placed,
//! and neither a label that lost a collision nor one behind the globe can be found.
#![allow(clippy::expect_used, clippy::panic)]

use geozero::{mvt::Message, FeatureProcessor, GeomProcessor};

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
    let coords = WorldTileCoords {
        x: 0,
        y: 0,
        z: ZoomLevel::from(0),
    };
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
