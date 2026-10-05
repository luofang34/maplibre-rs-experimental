//! Road names along their lines and text-fitted route shields, from vector tiles, keep their
//! glyphs and sprites at both pixel ratios, when zoomed and rotated, on the globe and over
//! terrain.

use geozero::mvt::{tile, Message as _};

use super::{count, AssetServer, SymbolMap, FONT, GLYPHS, SHIELD, SIZE, SPRITE};
use crate::{coords::Zoom, style::Style};

const NAME: [u8; 3] = [255, 0, 0];
const ROUTE: [u8; 3] = [0, 0, 255];
const WHOLE: [u32; 4] = [0, 0, SIZE, SIZE];

/// Every tile: a road across its middle, named and numbered. The fixtures look at the middle
/// of the zoom-14 tile row just north of the equator, so a road runs through the view's center.
fn road_tile() -> Vec<u8> {
    let string = |value: &str| tile::Value {
        string_value: Some(value.to_owned()),
        ..Default::default()
    };
    geozero::mvt::Tile {
        layers: vec![tile::Layer {
            version: 2,
            name: "roads".into(),
            extent: Some(4096),
            keys: vec!["name".into(), "ref".into()],
            values: vec![string("Main Street"), string("A1")],
            features: vec![tile::Feature {
                r#type: Some(tile::GeomType::Linestring as i32),
                tags: vec![0, 0, 1, 1],
                // From (0, 2048) to (4096, 2048), zigzag encoded.
                geometry: vec![9, 0, 4096, 10, 8192, 0],
                ..Default::default()
            }],
        }],
    }
    .encode_to_vec()
}

/// A terrarium DEM 500 m high everywhere, 256 pixels across as its source declares.
pub(super) fn dem_tile() -> Vec<u8> {
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(256, 256, image::Rgba([129, 244, 0, 255]))
        .write_to(&mut png, image::ImageFormat::Png)
        .expect("PNG");
    png.into_inner()
}

#[derive(Clone, Copy, Debug)]
struct Scene {
    pixel_ratio: f64,
    globe: bool,
    terrain: bool,
}

fn style(scene: Scene) -> Style {
    let mut style = serde_json::json!({
        "version":8,"center":[0.01,0.010986328],"zoom":14,"pitch":if scene.terrain { 45 } else { 0 },
        "glyphs":GLYPHS,"sprite":SPRITE,
        "sources":{
            "streets":{"type":"vector","tiles":["https://tiles.test/{z}/{x}/{y}.pbf"],"maxzoom":14},
            "dem":{"type":"raster-dem","tiles":["https://dem.test/{z}/{x}/{y}.png"],
                "encoding":"terrarium","maxzoom":12,"tileSize":256}
        },
        "layers":[
            {"id":"background","type":"background","paint":{"background-color":"#223344"}},
            {"id":"road","type":"line","source":"streets","source-layer":"roads",
                "paint":{"line-color":"#808080","line-width":6}},
            {"id":"road-name","type":"symbol","source":"streets","source-layer":"roads",
                "layout":{"symbol-placement":"line","symbol-spacing":200,
                    "text-field":["get","name"],"text-font":[FONT],"text-size":18,
                    "text-allow-overlap":true},
                "paint":{"text-color":"#ff0000"}},
            {"id":"shield","type":"symbol","source":"streets","source-layer":"roads",
                "layout":{"symbol-placement":"line","symbol-spacing":200,
                    "text-field":["get","ref"],"text-font":[FONT],"text-size":14,
                    "text-rotation-alignment":"viewport","icon-rotation-alignment":"viewport",
                    "icon-image":"shield","icon-text-fit":"both",
                    "icon-text-fit-padding":[2,4,2,4],"text-allow-overlap":true,
                    "icon-allow-overlap":true},
                "paint":{"text-color":"#0000ff"}}
        ]
    });
    if scene.globe {
        style["projection"] = serde_json::json!({"type":"vertical-perspective"});
    }
    if scene.terrain {
        style["terrain"] = serde_json::json!({"source":"dem","exaggeration":1});
    }
    serde_json::from_value(style).expect("style")
}

/// The texts of `layer`'s labels whose pixels of `color` are drawn, picked at those pixels in
/// screen points, `pixel_ratio` physical pixels each.
fn texts(
    map: &SymbolMap,
    (pixels, pixel_ratio): (&[u8], f64),
    color: [u8; 3],
    layer: &str,
) -> Vec<String> {
    let mut texts: Vec<String> = count(pixels, color, WHOLE)
        .iter()
        .step_by(5)
        .flat_map(|[x, y]| {
            let point = [f64::from(*x) / pixel_ratio, f64::from(*y) / pixel_ratio];
            map.map.query_rendered_symbols(point, Some(&[layer]))
        })
        .map(|hit| hit.text)
        .collect();
    texts.sort();
    texts.dedup();
    texts
}

/// The height in physical pixels of the shield around the drawn pixel nearest the viewport's
/// center, measured down its column.
fn shield_height(pixels: &[u8]) -> u32 {
    let shield = count(pixels, SHIELD, WHOLE);
    let center = f64::from(SIZE) / 2.0;
    let [x, y] = *shield
        .iter()
        .min_by(|a, b| {
            let distance = |[x, y]: [u32; 2]| (f64::from(x) - center).hypot(f64::from(y) - center);
            distance(**a).total_cmp(&distance(**b))
        })
        .expect("a shield is drawn");
    let column: Vec<u32> = shield
        .iter()
        .filter(|[sx, _]| *sx == x)
        .map(|[_, sy]| *sy)
        .collect();
    let (mut top, mut bottom) = (y, y);
    while column.contains(&(top.wrapping_sub(1))) {
        top -= 1;
    }
    while column.contains(&(bottom + 1)) {
        bottom += 1;
    }
    bottom - top + 1
}

/// The settled frame shows road names and shields from the served font and sprite.
fn check(map: &SymbolMap, (pixels, pixel_ratio): (&[u8], f64), case: &str) {
    assert!(
        map.server
            .glyph_requests()
            .iter()
            .any(|url| url == "https://glyphs.test/Noto%20Sans%20Regular/0-255.pbf"),
        "{case}: the labels' font is requested"
    );
    assert!(
        map.server
            .requested()
            .iter()
            .any(|url| url == "https://sprites.test/sprite.png"),
        "{case}: the shield's sprite is requested"
    );
    assert_eq!(
        texts(map, (pixels, pixel_ratio), NAME, "road-name"),
        ["Main Street"],
        "{case}"
    );
    assert_eq!(
        texts(map, (pixels, pixel_ratio), ROUTE, "shield"),
        ["A1"],
        "{case}"
    );
    let shield = count(pixels, SHIELD, WHOLE).len();
    assert!(shield > 100, "{case}: shields drawn with {shield} pixels");
}

async fn scene_map(scene: Scene) -> SymbolMap {
    let server = AssetServer::default();
    server.serve("https://tiles.test/", road_tile());
    server.serve("https://dem.test/", dem_tile());
    let mut map = SymbolMap::serving(style(scene), server).await;
    map.map.set_pixel_ratio(scene.pixel_ratio);
    map
}

#[tokio::test]
async fn road_names_and_shields_draw_at_both_pixel_ratios_zoomed_and_rotated() {
    let mut heights = Vec::new();
    for pixel_ratio in [1.0, 2.0] {
        let scene = Scene {
            pixel_ratio,
            globe: false,
            terrain: false,
        };
        let mut map = scene_map(scene).await;
        let pixels = map.settle().await;
        check(&map, (&pixels, pixel_ratio), &format!("{scene:?}"));
        heights.push(shield_height(&pixels));
        // Zooming in past the source's last zoom lays the tile out again, magnified.
        map.map.view_state_mut().zoom_to(Zoom::new(15.5));
        let pixels = map.settle().await;
        check(
            &map,
            (&pixels, pixel_ratio),
            &format!("{scene:?} at zoom 15.5"),
        );
        map.map
            .view_state_mut()
            .camera_mut()
            .set_bearing(cgmath::Deg(35.0));
        let pixels = map.settle().await;
        check(&map, (&pixels, pixel_ratio), &format!("{scene:?} rotated"));
    }
    let [one, two] = heights[..] else {
        panic!("two ratios");
    };
    assert!(
        f64::from(two) / f64::from(one) > 1.7 && f64::from(two) / f64::from(one) < 2.3,
        "a shield twice as dense is twice as tall: {one} px at 1, {two} px at 2"
    );
}

#[tokio::test]
async fn road_names_and_shields_draw_on_the_globe_and_over_terrain() {
    for (globe, terrain) in [(true, false), (false, true), (true, true)] {
        for pixel_ratio in [1.0, 2.0] {
            let scene = Scene {
                pixel_ratio,
                globe,
                terrain,
            };
            let mut map = scene_map(scene).await;
            let pixels = map.settle().await;
            check(&map, (&pixels, pixel_ratio), &format!("{scene:?}"));
            map.map
                .view_state_mut()
                .camera_mut()
                .set_bearing(cgmath::Deg(-25.0));
            let pixels = map.settle().await;
            check(&map, (&pixels, pixel_ratio), &format!("{scene:?} rotated"));
        }
    }
}
