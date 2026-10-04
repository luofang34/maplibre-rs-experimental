//! A street name that stands upright to the viewer follows the curve of its street, glyph by
//! glyph, as the screen shows the street, at any pitch and bearing.

use geozero::mvt::{tile, Message as _};

use super::{count, AssetServer, SymbolMap, FONT, GLYPHS, SIZE, SPRITE};
use crate::style::Style;

const ROAD: [u8; 3] = [128, 128, 128];

/// Every tile: an arch of radius 1200 tile units around the point the view looks at, from
/// 200 to 340 degrees, named `name`.
fn arch_tile(name: &str) -> Vec<u8> {
    let zigzag = |value: i32| ((value << 1) ^ (value >> 31)) as u32;
    let points: Vec<[i32; 2]> = (0..=56)
        .map(|step| {
            let angle = (200.0 + 140.0 * f64::from(step) / 56.0).to_radians();
            [
                (1864.0 + 1200.0 * angle.cos()).round() as i32,
                (2048.0 + 1200.0 * angle.sin()).round() as i32,
            ]
        })
        .collect();
    let mut geometry = vec![9, zigzag(points[0][0]), zigzag(points[0][1])];
    geometry.push(((points.len() as u32 - 1) << 3) | 2);
    for pair in points.windows(2) {
        geometry.push(zigzag(pair[1][0] - pair[0][0]));
        geometry.push(zigzag(pair[1][1] - pair[0][1]));
    }
    geozero::mvt::Tile {
        layers: vec![tile::Layer {
            version: 2,
            name: "roads".into(),
            extent: Some(4096),
            keys: vec!["name".into()],
            values: vec![tile::Value {
                string_value: Some(name.into()),
                ..Default::default()
            }],
            features: vec![tile::Feature {
                r#type: Some(tile::GeomType::Linestring as i32),
                tags: vec![0, 0],
                geometry,
                ..Default::default()
            }],
        }],
    }
    .encode_to_vec()
}

fn style(pitch: f64, bearing: f64) -> Style {
    serde_json::from_value(serde_json::json!({
        "version":8,"center":[0.01,0.010986328],"zoom":14,"pitch":pitch,"bearing":bearing,
        "glyphs":GLYPHS,"sprite":SPRITE,
        "sources":{"streets":{"type":"vector","tiles":["https://tiles.test/{z}/{x}/{y}.pbf"],"maxzoom":14}},
        "layers":[
            {"id":"background","type":"background","paint":{"background-color":"#223344"}},
            {"id":"road","type":"line","source":"streets","source-layer":"roads",
                "paint":{"line-color":"#808080","line-width":3}},
            {"id":"road-name","type":"symbol","source":"streets","source-layer":"roads",
                "layout":{"symbol-placement":"line","symbol-spacing":250,
                    "text-field":["get","name"],"text-font":[FONT],"text-size":20,
                    "text-rotation-alignment":"map","text-pitch-alignment":"viewport",
                    "text-keep-upright":true,"text-max-angle":90,"text-allow-overlap":true},
                "paint":{"text-color":"#ff0000"}}
        ]
    }))
    .expect("style")
}

/// The glyph centres of the street name placed nearest the middle of the view.
fn glyph_centres(map: &SymbolMap) -> Vec<[f64; 2]> {
    let placed = map
        .map
        .map_context
        .world
        .resources
        .get::<crate::sdf::query::PlacedSymbols>()
        .expect("placed symbols");
    let middle = f64::from(SIZE) / 2.0;
    let centre = |area: &[f64; 4]| [(area[0] + area[2]) / 2.0, (area[1] + area[3]) / 2.0];
    placed
        .0
        .iter()
        .filter(|symbol| symbol.layer == "road-name" && symbol.glyph_boxes.len() >= 8)
        .map(|symbol| symbol.glyph_boxes.iter().map(centre).collect::<Vec<_>>())
        .min_by(|a, b| {
            let off = |glyphs: &Vec<[f64; 2]>| {
                let [x, y] = glyphs[glyphs.len() / 2];
                (x - middle).hypot(y - middle)
            };
            off(a).total_cmp(&off(b))
        })
        .expect("a street name placed glyph by glyph")
}

/// How far `point` lies from the line through `from` and `to`.
fn off_chord(point: [f64; 2], from: [f64; 2], to: [f64; 2]) -> f64 {
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    ((point[0] - from[0]) * dy - (point[1] - from[1]) * dx).abs() / dx.hypot(dy)
}

async fn arch_map(name: &str, pitch: f64, bearing: f64) -> SymbolMap {
    let server = AssetServer::default();
    server.serve("https://tiles.test/", arch_tile(name));
    SymbolMap::serving(style(pitch, bearing), server).await
}

#[tokio::test]
async fn a_street_name_upright_to_the_viewer_follows_its_curving_street() {
    // Turned half way around, the street runs leftward and its name flips to read upright.
    for (pitch, bearing) in [(0.0, 0.0), (45.0, 20.0), (30.0, 180.0)] {
        let case = format!("pitch {pitch}, bearing {bearing}");
        let mut map = arch_map("Revere Road", pitch, bearing).await;
        map.settle().await;
        let glyphs = glyph_centres(&map);
        let (first, last) = (glyphs[0], glyphs[glyphs.len() - 1]);
        assert!(
            first[0] < last[0],
            "{case}: reads left to right: {glyphs:?}"
        );
        let bend = glyphs
            .iter()
            .map(|glyph| off_chord(*glyph, first, last))
            .fold(0.0, f64::max);
        assert!(
            bend > 4.0,
            "{case}: the name bends with its street by {bend} px: {glyphs:?}"
        );
        for pair in glyphs.windows(2) {
            let step = (pair[1][0] - pair[0][0]).hypot(pair[1][1] - pair[0][1]);
            // The word space has no glyph of its own, so the step across it is the widest.
            assert!(
                (4.0..30.0).contains(&step),
                "{case}: glyphs {step} px apart: {glyphs:?}"
            );
        }
        // Every glyph's centre lies on the street as the frame draws it without its name.
        map.map
            .mutate_style(|style| {
                style.set_layout_property("road-name", "visibility", "none".into())
            })
            .expect("hide the name");
        let pixels = map.settle().await;
        let road = count(&pixels, ROAD, [0, 0, SIZE, SIZE]);
        for glyph in &glyphs {
            let nearest = road
                .iter()
                .map(|[x, y]| {
                    (f64::from(*x) + 0.5 - glyph[0]).hypot(f64::from(*y) + 0.5 - glyph[1])
                })
                .fold(f64::INFINITY, f64::min);
            assert!(
                nearest < 2.5,
                "{case}: glyph at {glyph:?} is {nearest} px off the street"
            );
        }
    }
}

/// For each inner glyph of an all-`I` name: how far, in degrees, its stroke leans from square
/// to its street (the long axis of its pixels against the normal of the line through its
/// neighbours' centres), and how tall the stroke is drawn, in pixels.
fn strokes(pixels: &[u8], glyphs: &[[f64; 2]]) -> Vec<(f64, f64)> {
    let text = count(pixels, [255, 0, 0], [0, 0, SIZE, SIZE]);
    glyphs
        .windows(3)
        .map(|three| {
            let (dx, dy) = (three[2][0] - three[0][0], three[2][1] - three[0][1]);
            let (ux, uy) = (dx / dx.hypot(dy), dy / dx.hypot(dy));
            let (mut uu, mut vv, mut uv) = (0.0, 0.0, 0.0);
            let (mut low, mut high) = (f64::INFINITY, f64::NEG_INFINITY);
            for [x, y] in &text {
                let (ox, oy) = (
                    f64::from(*x) + 0.5 - three[1][0],
                    f64::from(*y) + 0.5 - three[1][1],
                );
                let (u, v) = (ox * ux + oy * uy, oy * ux - ox * uy);
                if ox.hypot(oy) <= 7.0 {
                    uu += u * u;
                    vv += v * v;
                    uv += u * v;
                }
                if u.abs() < 2.0 && v.abs() < 15.0 {
                    low = low.min(v);
                    high = high.max(v);
                }
            }
            let axis = 0.5 * (2.0 * uv).atan2(uu - vv);
            ((axis.to_degrees().abs() - 90.0).abs(), high - low + 1.0)
        })
        .collect()
}

/// The mean distance between neighbouring glyph centres over the mean stroke height: the
/// spacing the glyphs are placed at against the size they are drawn at.
fn spacing_over_height(glyphs: &[[f64; 2]], strokes: &[(f64, f64)]) -> f64 {
    let steps: Vec<f64> = glyphs
        .windows(2)
        .map(|pair| (pair[1][0] - pair[0][0]).hypot(pair[1][1] - pair[0][1]))
        .collect();
    let step = steps.iter().sum::<f64>() / steps.len() as f64;
    let height = strokes.iter().map(|stroke| stroke.1).sum::<f64>() / strokes.len() as f64;
    step / height
}

#[tokio::test]
async fn each_glyph_stands_square_to_its_street_at_the_size_it_is_spaced_for() {
    let mut level = None;
    for (pitch, bearing) in [(0.0, 0.0), (45.0, 20.0), (60.0, 0.0)] {
        let mut map = arch_map("IIIIIIIIIIIIII", pitch, bearing).await;
        let pixels = map.settle().await;
        let glyphs = glyph_centres(&map);
        let strokes = strokes(&pixels, &glyphs);
        let leans: Vec<f64> = strokes.iter().map(|stroke| stroke.0).collect();
        let mean = leans.iter().sum::<f64>() / leans.len() as f64;
        assert!(
            mean < 3.5 && leans.iter().all(|lean| *lean < 10.0),
            "pitch {pitch}: strokes lean from square by {leans:.1?} degrees"
        );
        // Glyphs drawn smaller or larger with distance are spaced closer or wider with them.
        let ratio = spacing_over_height(&glyphs, &strokes);
        let flat = *level.get_or_insert(ratio);
        assert!(
            (ratio / flat - 1.0).abs() < 0.03,
            "pitch {pitch}: spacing over stroke height {ratio}, {flat} seen flat"
        );
    }
}
