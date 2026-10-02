//! A label on a variable anchor keeps it from frame to frame while it still fits, as GL JS
//! keeps a symbol's previous anchor, rather than jumping to an earlier one that frees up.
#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use crate::sdf::query::QueryOptions;

const CENTER: [f64; 2] = [SIZE as f64 / 2.0, SIZE as f64 / 2.0];

fn style(bearing: f64) -> Style {
    serde_json::from_value(serde_json::json!({
        "version": 8, "center": [0.0439453125, -0.04394530819], "zoom": 12, "bearing": bearing,
        "sources": {
            "map": {"type": "vector", "tiles": ["https://unused.invalid/{z}/{x}/{y}"], "maxzoom": 12}
        },
        "layers": [
            {"id": "label", "type": "symbol", "source": "map", "source-layer": "places",
                "layout": {"text-field": "Moved", "text-size": 20,
                    "text-variable-anchor": ["left", "right"]}},
            {"id": "obstacle", "type": "symbol", "source": "map", "source-layer": "places",
                "layout": {"text-field": "Block", "text-size": 20}}
        ]
    }))
    .expect("variable anchor style")
}

/// The label at the tile's centre and, 480 tile units (60 pixels at zoom 12) to its east, the
/// obstacle a layer above it, which is placed first.
fn layers(style: &Style) -> crate::headless::map::ProcessedLayers {
    let coords = WorldTileCoords {
        x: 2048,
        y: 2048,
        z: ZoomLevel::from(12),
    };
    let atlas = atlas();
    let mut layers = crate::headless::map::ProcessedLayers::default();
    for (index, x) in [(0, 2048.0), (1, 2528.0)] {
        let Some(LayerPaint::Symbol(paint)) = &style.layers[index].paint else {
            panic!("symbol paint");
        };
        let mut layout = TextTessellator::default();
        layout.configure(paint.clone(), atlas.clone());
        layout.point_begin(0).expect("point begin");
        layout.xy(x, 2048.0, 0).expect("position");
        layout.point_end(0).expect("point end");
        layout.feature_end(0).expect("feature");
        layout.finish();
        layers
            .symbols
            .push(Box::new(DefaultSymbolLayerTessellated::build_from(
                coords,
                layout.quad_buffer.into(),
                layout.features,
                Some(atlas.clone()),
                geozero::mvt::tile::Layer {
                    name: "places".into(),
                    version: 2,
                    extent: Some(4096),
                    ..Default::default()
                },
                style.layers[index].id.clone(),
            )));
    }
    layers
}

fn label_at(map: &HeadlessMap, dx: f64) -> bool {
    map.query_rendered_symbols_in(
        crate::sdf::query::QueryGeometry::Point([CENTER[0] + dx, CENTER[1]]),
        &QueryOptions {
            layers: Some(vec!["label".to_owned()]),
            filter: None,
        },
    )
    .expect("query")
    .iter()
    .any(|symbol| symbol.text == "Moved")
}

#[tokio::test]
async fn a_label_keeps_its_anchor_while_it_still_fits() {
    let mut map = fixture_map(style(0.0), layers(&style(0.0)), 1).await;
    // The obstacle takes the space east of the point, so the label goes west of it.
    assert!(label_at(&map, -30.0), "the label takes its second anchor");
    assert!(!label_at(&map, 20.0), "nothing is east of the point");

    // Turning the map carries the obstacle north of the label, and the first anchor, east of
    // the point, would now fit.
    map.view_state_mut()
        .camera_mut()
        .set_bearing(cgmath::Deg(90.0));
    for _ in 0..8 {
        map.frame_input_mut()
            .advance(std::time::Duration::from_millis(16));
        map.run_frame().expect("turned frame");
        assert!(label_at(&map, -30.0), "the label stays on its anchor");
        assert!(!label_at(&map, 20.0), "the label does not jump back");
    }

    // A map that starts turned has no anchor to keep and takes the first.
    let fresh = fixture_map(style(90.0), layers(&style(90.0)), 1).await;
    assert!(
        label_at(&fresh, 20.0),
        "with no history the first anchor fits"
    );
    assert!(
        !label_at(&fresh, -30.0),
        "with no history the second is not needed"
    );
}
