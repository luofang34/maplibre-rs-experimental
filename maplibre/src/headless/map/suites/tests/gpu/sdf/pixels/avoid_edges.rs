//! Headless GPU pixel render test verifying `symbol-avoid-edges` suppresses labels crossing
//! tile boundaries (`[0, 4096]`) while preserving interior labels.

use super::*;

const PIXELS_PER_UNIT: f64 = 512.0 / 4096.0;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn avoid_style(avoid_edges: bool) -> TestResult<Style> {
    Ok(serde_json::from_value(serde_json::json!({
        "version": 8,
        "center": [0.0, 0.0],
        "zoom": 12,
        "sources": {
            "map": {
                "type": "vector",
                "tiles": ["https://unused.invalid/{z}/{x}/{y}"],
                "maxzoom": 12
            }
        },
        "layers": [
            {
                "id": "background",
                "type": "background",
                "paint": { "background-color": "#334455" }
            },
            {
                "id": "point",
                "type": "circle",
                "source": "map",
                "source-layer": "places",
                "paint": { "circle-radius": 0.1 }
            },
            {
                "id": "label",
                "type": "symbol",
                "source": "map",
                "source-layer": "places",
                "layout": {
                    "text-field": "Alps",
                    "text-size": 28,
                    "text-allow-overlap": true,
                    "symbol-avoid-edges": avoid_edges
                },
                "paint": { "text-color": "#ff0000" }
            }
        ]
    }))?)
}

async fn render_points(avoid_edges: bool, anchors: &[[f64; 2]]) -> TestResult<Vec<u8>> {
    let style = avoid_style(avoid_edges)?;
    let coords = WorldTileCoords {
        x: 2048,
        y: 2048,
        z: ZoomLevel::from(12),
    };
    let Some(LayerPaint::Symbol(paint)) = &style.layers[2].paint else {
        return Err("missing symbol paint".into());
    };
    let shared_atlas = atlas();
    let mut layout = TextTessellator::default();
    layout.configure(paint.clone(), shared_atlas.clone());
    for (idx, pt) in anchors.iter().enumerate() {
        layout.point_begin(idx)?;
        layout.xy(pt[0], pt[1], idx)?;
        layout.point_end(idx)?;
        layout.feature_end(idx as u64)?;
    }
    layout.finish();

    let base = geozero::mvt::Tile {
        layers: vec![geozero::mvt::tile::Layer {
            name: "places".into(),
            version: 2,
            extent: Some(4096),
            features: vec![geozero::mvt::tile::Feature {
                r#type: Some(1),
                geometry: vec![9, 0, 0],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
    .encode_to_vec();
    let mut layers = process_tile_layers(&base, &style.layers[1], coords, Default::default())?;
    if !layout.quad_buffer.indices.is_empty() {
        layers
            .symbols
            .push(Box::new(DefaultSymbolLayerTessellated::build_from(
                coords,
                layout.quad_buffer.into(),
                layout.features,
                Some(shared_atlas),
                geozero::mvt::tile::Layer {
                    name: "places".into(),
                    version: 2,
                    extent: Some(4096),
                    ..Default::default()
                },
                "label".into(),
            )));
    }
    let map = fixture_map(style, layers, 1).await;
    Ok(read_blocking(&map))
}

fn red_pixels_near(pixels: &[u8], screen_pt: [f64; 2], radius: f64) -> usize {
    pixels
        .as_chunks::<4>()
        .0
        .iter()
        .enumerate()
        .filter(|(_, p)| p[0] > 180 && p[1] < 80 && p[2] < 80)
        .filter(|(idx, _)| {
            let x = (*idx % SIZE as usize) as f64 + 0.5;
            let y = (*idx / SIZE as usize) as f64 + 0.5;
            (x - screen_pt[0]).hypot(y - screen_pt[1]) <= radius
        })
        .count()
}

#[test]
fn symbol_avoid_edges_suppresses_rendered_pixels_crossing_tile_boundary() -> TestResult {
    futures::executor::block_on(async {
        // Tile (2048, 2048, z12) has its top-left corner (0, 0) at screen centre (256, 256).
        // Anchor [30, 600] crosses the left tile boundary x=0; anchor [1200, 1200] sits well inside.
        let anchors = [[30.0, 600.0], [1200.0, 1200.0]];
        let edge_screen = [
            256.0 + 30.0 * PIXELS_PER_UNIT,
            256.0 + 600.0 * PIXELS_PER_UNIT,
        ];
        let interior_screen = [
            256.0 + 1200.0 * PIXELS_PER_UNIT,
            256.0 + 1200.0 * PIXELS_PER_UNIT,
        ];

        let unclipped = render_points(false, &anchors).await?;
        assert!(
            red_pixels_near(&unclipped, edge_screen, 45.0) > 20,
            "edge label renders red glyph pixels when symbol-avoid-edges is false"
        );
        assert!(
            red_pixels_near(&unclipped, interior_screen, 45.0) > 20,
            "interior label renders red glyph pixels when symbol-avoid-edges is false"
        );

        let clipped = render_points(true, &anchors).await?;
        assert_eq!(
            red_pixels_near(&clipped, edge_screen, 45.0),
            0,
            "edge-crossing label draws zero pixels when symbol-avoid-edges is true"
        );
        assert!(
            red_pixels_near(&clipped, interior_screen, 45.0) > 20,
            "interior label still renders when symbol-avoid-edges is true"
        );
        Ok(())
    })
}
