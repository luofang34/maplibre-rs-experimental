//! A road name along its road over terrain keeps every glyph: each glyph lies on the ground
//! under it, and the label is hidden only as a whole, where ground stands in front of its
//! anchor.

mod hill;
mod slope;

use super::{dem::serve_dem, AssetServer, SymbolMap, FONT, GLYPHS, SPRITE};
use crate::{sdf::query::PlacedSymbols, style::Style};

const NAME: [u8; 3] = [255, 0, 0];
/// The view's center: the middle of a zoom-14 tile just north of the equator, which the road
/// of [`road_tile`] crosses from west to east.
const CENTER: [f64; 2] = [0.010986328125, 0.010986328];
/// The ground's height at the view's center.
const BASE_METRES: f64 = 1000.0;
/// The equator's length, which a mercator `x` of 0..1 spans; near the equator a mercator `y`
/// of 0..1 spans as much.
const EQUATOR_METRES: f64 = 40_075_016.686;

/// Every tile: a road across its middle from west to east, named.
fn road_tile() -> Vec<u8> {
    use geozero::mvt::{tile, Message as _};
    geozero::mvt::Tile {
        layers: vec![tile::Layer {
            version: 2,
            name: "roads".into(),
            extent: Some(4096),
            keys: vec!["name".into()],
            values: vec![tile::Value {
                string_value: Some("Burnside Lubni".to_owned()),
                ..Default::default()
            }],
            features: vec![tile::Feature {
                r#type: Some(tile::GeomType::Linestring as i32),
                tags: vec![0, 0],
                // From (0, 2048) to (4096, 2048), zigzag encoded.
                geometry: vec![9, 0, 4096, 10, 8192, 0],
                ..Default::default()
            }],
        }],
    }
    .encode_to_vec()
}

/// How the view looks at the road.
#[derive(Clone, Copy, Debug)]
struct View {
    pitch: f64,
    bearing: f64,
    ratio: f64,
    /// The zoom of the DEM's finest tiles.
    dem_zoom: u32,
    /// Whether imagery is drawn under the road.
    imagery: bool,
}

const VIEW: View = View {
    pitch: 50.0,
    bearing: 0.0,
    ratio: 1.0,
    dem_zoom: 12,
    imagery: false,
};

fn style(view: View) -> Style {
    let mut layers = vec![serde_json::json!({"id":"background","type":"background",
        "paint":{"background-color":"#223344"}})];
    if view.imagery {
        layers.push(serde_json::json!({"id":"imagery","type":"raster","source":"imagery"}));
    }
    layers.extend([
        serde_json::json!({"id":"road","type":"line","source":"streets","source-layer":"roads",
            "paint":{"line-color":"#808080","line-width":4}}),
        serde_json::json!({"id":"road-name","type":"symbol","source":"streets",
            "source-layer":"roads",
            "layout":{"symbol-placement":"line-center","text-field":["get","name"],
                "text-font":[FONT],"text-size":18,"text-allow-overlap":true},
            "paint":{"text-color":"#ff0000"}}),
    ]);
    serde_json::from_value(serde_json::json!({
        "version":8,"center":CENTER,"zoom":15.0 - view.ratio.log2(),"pitch":view.pitch,
        "bearing":view.bearing,"glyphs":GLYPHS,"sprite":SPRITE,
        "sources":{
            "streets":{"type":"vector","tiles":["https://tiles.test/{z}/{x}/{y}.pbf"],"maxzoom":14},
            "dem":{"type":"raster-dem","tiles":["https://dem.test/{z}/{x}/{y}.png"],
                "encoding":"terrarium","maxzoom":view.dem_zoom,"tileSize":256},
            "imagery":{"type":"raster","tiles":["https://imagery.test/{z}/{x}/{y}.png"],
                "tileSize":256}
        },
        "terrain":{"source":"dem","exaggeration":1},
        "layers":layers
    }))
    .expect("style")
}

/// Metres east and south of the view's center at the world's mercator `x` and `y`.
fn from_center(x: f64, y: f64) -> [f64; 2] {
    let center_x = (CENTER[0] + 180.0) / 360.0;
    let center_y = 0.5 - CENTER[1].to_radians().tan().asinh() / std::f64::consts::TAU;
    [
        (x - center_x) * EQUATOR_METRES,
        (y - center_y) * EQUATOR_METRES,
    ]
}

/// The map of `view` over `ground`, its height in metres at points east and south of the
/// view's center.
async fn scene_map(view: View, ground: &dyn Fn([f64; 2]) -> f64) -> SymbolMap {
    let server = AssetServer::default();
    server.serve("https://tiles.test/", road_tile());
    let mut imagery = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(256, 256, image::Rgba([40, 120, 40, 255]))
        .write_to(&mut imagery, image::ImageFormat::Png)
        .expect("PNG");
    server.serve("https://imagery.test/", imagery.into_inner());
    // The DEM tiles around the view's center at the DEM's finest zoom, 2048 x 2047 at zoom 12.
    let [x, y] = [2048_u32, 2047].map(|tile| tile << (view.dem_zoom - 12));
    let reach = 2 << (view.dem_zoom - 12);
    serve_dem(
        &server,
        view.dem_zoom,
        [x - reach, y - reach, x + reach, y + reach],
        &|x, y| ground(from_center(x, y)).max(0.0),
    );
    let mut map = SymbolMap::serving(style(view), server).await;
    map.map.set_pixel_ratio(view.ratio);
    map
}

/// The glyph boxes, in layout pixels, of the road name nearest the view's center.
fn middle_label_glyphs(map: &SymbolMap) -> Vec<[f64; 4]> {
    let placed = map
        .map
        .map_context
        .world
        .resources
        .get::<PlacedSymbols>()
        .expect("placed symbols");
    let view = map.map.view_state();
    let middle = [view.width() / 2.0, view.height() / 2.0];
    let off = |boxes: &Vec<[f64; 4]>| {
        let [l, t, r, b] = boxes[boxes.len() / 2];
        ((l + r) / 2.0 - middle[0]).hypot((t + b) / 2.0 - middle[1])
    };
    placed
        .0
        .iter()
        .filter(|symbol| symbol.layer == "road-name" && !symbol.glyph_boxes.is_empty())
        .min_by(|a, b| off(&a.glyph_boxes).total_cmp(&off(&b.glyph_boxes)))
        .expect("the road name is placed glyph by glyph")
        .glyph_boxes
        .clone()
}

/// The text pixels inside each glyph box of the road name nearest the view's center, at
/// `ratio` device pixels per layout pixel.
fn glyph_pixels(map: &SymbolMap, pixels: &[u8], ratio: f64) -> Vec<usize> {
    let width = map.map.head_texture().expect("color").width() as usize;
    let span = |low: f64, high: f64| {
        (low * ratio).floor().max(0.0) as usize..(high * ratio).ceil().max(0.0) as usize
    };
    middle_label_glyphs(map)
        .iter()
        .map(|[l, t, r, b]| {
            span(*t, *b)
                .flat_map(|y| span(*l, *r).map(move |x| (x, y)))
                .filter(|(x, y)| {
                    let index = (y * width + x) * 4;
                    pixels.get(index..index + 3).is_some_and(|pixel| {
                        pixel
                            .iter()
                            .zip(NAME)
                            .all(|(have, want)| have.abs_diff(want) < 60)
                    })
                })
                .count()
        })
        .collect()
}

/// The glyphs of `case` that show less than `share` of the same glyph in `reference`.
fn incomplete_glyphs(case: &str, glyphs: &[usize], reference: &[usize], share: f64) -> Vec<String> {
    let mut failures = Vec::new();
    if glyphs.len() != reference.len() {
        failures.push(format!(
            "{case}: {} glyphs against {}",
            glyphs.len(),
            reference.len()
        ));
    }
    for (index, (have, whole)) in glyphs.iter().zip(reference).enumerate() {
        if (*have as f64) < share * *whole as f64 {
            failures.push(format!(
                "{case}: glyph {index} shows {have} pixels against {whole}: {glyphs:?}"
            ));
        }
    }
    failures
}
