//! Labels that follow a line: glyphs on a curve, reading direction, bends and repeats.
use super::*;

/// Tile units span 512 pixels per 4096 at zoom 12; the view centre is the tile's corner.
const PIXELS_PER_UNIT: f64 = 512.0 / 4096.0;

fn line_style(layout: serde_json::Value) -> Style {
    let mut layout_value = serde_json::json!({
        "symbol-placement": "line-center", "text-field": "Alps", "text-size": 28,
        "text-allow-overlap": true, "text-max-angle": 180
    });
    for (key, value) in layout.as_object().expect("layout").clone() {
        layout_value[key] = value;
    }
    serde_json::from_value(serde_json::json!({
        "version":8, "center":[0.0, 0.0], "zoom":12,
        "sources":{"map":{"type":"vector","tiles":["https://unused.invalid/{z}/{x}/{y}"],"maxzoom":12}},
        "layers":[
            {"id":"background","type":"background","paint":{"background-color":"#334455"}},
            {"id":"point","type":"circle","source":"map","source-layer":"places","paint":{"circle-radius":0.1}},
            {"id":"label","type":"symbol","source":"map","source-layer":"roads",
                "layout":layout_value,
                "paint":{"text-color":"#ff0000"}}
        ]
    }))
    .expect("line label style")
}

async fn line_map(style: Style, line: &[[f64; 2]]) -> HeadlessMap {
    let coords = WorldTileCoords {
        x: 2048,
        y: 2048,
        z: ZoomLevel::from(12),
    };
    let Some(LayerPaint::Symbol(paint)) = &style.layers[2].paint else {
        panic!("symbol paint");
    };
    let atlas = atlas();
    let mut layout = TextTessellator::default();
    layout.configure(paint.clone(), atlas.clone());
    layout.linestring_begin(true, line.len(), 0).expect("begin");
    for (index, point) in line.iter().enumerate() {
        layout.xy(point[0], point[1], index).expect("vertex");
    }
    layout.linestring_end(true, 0).expect("end");
    layout.feature_end(0).expect("feature");
    layout.finish();
    // A vector layer on the tile makes it available, so its symbols are placed.
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
    let mut layers =
        process_tile_layers(&base, &style.layers[1], coords, Default::default()).expect("base");
    if !layout.quad_buffer.indices.is_empty() {
        layers
            .symbols
            .push(Box::new(DefaultSymbolLayerTessellated::build_from(
                coords,
                layout.quad_buffer.into(),
                layout.features,
                Some(atlas),
                geozero::mvt::tile::Layer {
                    name: "roads".into(),
                    version: 2,
                    extent: Some(4096),
                    ..Default::default()
                },
                "label".into(),
            )));
    }
    fixture_map(style, layers, 1).await
}

/// Red text pixels as distances from a screen point.
fn distances_from(pixels: &[u8], centre: [f64; 2]) -> Vec<f64> {
    pixels
        .chunks_exact(4)
        .enumerate()
        .filter(|(_, p)| p[0] > 180 && p[1] < 80 && p[2] < 80)
        .map(|(index, _)| {
            let (x, y) = (
                (index % SIZE as usize) as f64,
                (index / SIZE as usize) as f64,
            );
            (x + 0.5 - centre[0]).hypot(y + 0.5 - centre[1])
        })
        .collect()
}

fn arc(centre: [f64; 2], radius: f64, from: f64, to: f64) -> Vec<[f64; 2]> {
    (0..=120)
        .map(|step| {
            let angle = (from + (to - from) * f64::from(step) / 120.0).to_radians();
            [
                centre[0] + radius * angle.cos(),
                centre[1] + radius * angle.sin(),
            ]
        })
        .collect()
}

#[tokio::test]
async fn a_label_on_an_arc_bends_with_it() {
    // A 60 px circle whose top arc carries the label; the centre is at screen (381, 381).
    let centre_units = [1000.0, 1000.0];
    let radius_units = 480.0;
    let screen_centre = [
        256.0 + centre_units[0] * PIXELS_PER_UNIT,
        256.0 + centre_units[1] * PIXELS_PER_UNIT,
    ];
    let line = arc(centre_units, radius_units, 200.0, 340.0);
    let map = line_map(line_style(serde_json::json!({})), &line).await;
    let distances = distances_from(&read_blocking(&map), screen_centre);
    assert!(
        distances.len() > 100,
        "the label drew {} pixels",
        distances.len()
    );
    let radius = radius_units * PIXELS_PER_UNIT;
    let (nearest, farthest) = distances
        .iter()
        .fold((f64::MAX, 0.0_f64), |(a, b), d| (a.min(*d), b.max(*d)));
    // Glyphs on the arc stay within their own height of it (about 26 px for this text, from
    // descenders inward to ascenders outward); a straight baseline would push the outer
    // glyphs to about 80 px.
    assert!(
        nearest > radius - 20.0 && farthest < radius + 14.0,
        "text spans {nearest:.1}..{farthest:.1} px from the centre of a {radius} px arc"
    );
}

fn red_mask(pixels: &[u8]) -> Vec<bool> {
    pixels
        .chunks_exact(4)
        .map(|p| p[0] > 180 && p[1] < 80 && p[2] < 80)
        .collect()
}

fn differing(a: &[bool], b: &[bool]) -> usize {
    a.iter().zip(b).filter(|(a, b)| a != b).count()
}

fn top_arc() -> Vec<[f64; 2]> {
    arc([1000.0, 1000.0], 480.0, 200.0, 340.0)
}

#[tokio::test]
async fn a_line_drawn_backwards_still_reads_upright() {
    let forward = red_mask(&read_blocking(
        &line_map(line_style(serde_json::json!({})), &top_arc()).await,
    ));
    let mut reversed_line = top_arc();
    reversed_line.reverse();
    let reversed = red_mask(&read_blocking(
        &line_map(line_style(serde_json::json!({})), &reversed_line).await,
    ));
    let ink = forward.iter().filter(|red| **red).count();
    assert!(ink > 100, "the label drew {ink} pixels");
    assert!(
        differing(&forward, &reversed) * 10 < ink,
        "keep-upright makes both directions draw alike: {} of {ink} pixels differ",
        differing(&forward, &reversed)
    );
    let unturned = line_style(serde_json::json!({"text-keep-upright": false}));
    let backwards = red_mask(&read_blocking(&line_map(unturned, &reversed_line).await));
    assert!(
        differing(&forward, &backwards) * 2 > ink,
        "without keep-upright the reversed line writes the text upside down"
    );
}

#[tokio::test]
async fn a_sharp_bend_under_the_label_refuses_it_until_max_angle_allows_it() {
    let corner = [[900.0, 1000.0], [1400.0, 1000.0], [1400.0, 1500.0]];
    // The corner lies at the middle of this line, under the centre of the label.
    let refuses = line_style(serde_json::json!({"text-max-angle": 45}));
    let map = line_map(refuses, &corner).await;
    assert!(
        !red_mask(&read_blocking(&map)).iter().any(|red| *red),
        "a 90 degree corner under the label leaves it out"
    );
    let allows = line_style(serde_json::json!({"text-max-angle": 100}));
    let map = line_map(allows, &corner).await;
    assert!(
        red_mask(&read_blocking(&map))
            .iter()
            .filter(|red| **red)
            .count()
            > 100
    );
}

#[tokio::test]
async fn a_curved_label_is_selectable_on_its_glyphs_but_not_under_the_curve() {
    let map = line_map(line_style(serde_json::json!({})), &top_arc()).await;
    let pixels = read_blocking(&map);
    let index = pixels
        .chunks_exact(4)
        .position(|p| p[0] > 180 && p[1] < 80 && p[2] < 80)
        .expect("the label draws");
    let on_glyph = [
        (index % SIZE as usize) as f64 + 0.5,
        (index / SIZE as usize) as f64 + 0.5,
    ];
    assert!(
        !map.query_rendered_symbols(on_glyph, None).is_empty(),
        "a drawn glyph selects the label"
    );
    // Below the middle of the arc, inside the box around all glyphs but on none of them.
    assert!(
        map.query_rendered_symbols([381.0, 340.0], None).is_empty(),
        "the space under a curve is not part of the label"
    );
}
