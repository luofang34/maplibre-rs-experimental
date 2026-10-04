//! GeoJSON labels, whose layers name no source layer, get the font and sprite they ask for.

use std::sync::Arc;

use super::{count, SymbolMap, FONT, GLYPHS, MARKER, SIZE, SPRITE};
use crate::style::{source::GeoJsonData, Style};

pub(super) const TEXT: [u8; 3] = [255, 0, 0];

pub(super) fn point(name: Option<&str>, icon: Option<&str>, at: [f64; 2]) -> serde_json::Value {
    serde_json::json!({"type":"Feature",
        "properties":{"name":name,"icon":icon},
        "geometry":{"type":"Point","coordinates":at}})
}

pub(super) fn collection(features: Vec<serde_json::Value>) -> serde_json::Value {
    serde_json::json!({"type":"FeatureCollection","features":features})
}

pub(super) fn style(data: serde_json::Value) -> Style {
    serde_json::from_value(serde_json::json!({
        "version":8,"center":[0,0],"zoom":3,
        "glyphs":GLYPHS,"sprite":SPRITE,
        "sources":{"places":{"type":"geojson","data":data}},
        "layers":[
            {"id":"background","type":"background","paint":{"background-color":"#223344"}},
            {"id":"labels","type":"symbol","source":"places",
                "layout":{"text-field":["get","name"],"text-font":[FONT],"text-size":28,
                    "icon-image":["get","icon"],"text-allow-overlap":true,
                    "icon-allow-overlap":true},
                "paint":{"text-color":"#ff0000"}}
        ]
    }))
    .expect("style")
}

/// Viewport quarters: `[left, top, right, bottom]`.
pub(super) const NORTH_WEST: [u32; 4] = [0, 0, SIZE / 2, SIZE / 2];
pub(super) const NORTH_EAST: [u32; 4] = [SIZE / 2, 0, SIZE, SIZE / 2];
pub(super) const SOUTH_WEST: [u32; 4] = [0, SIZE / 2, SIZE / 2, SIZE];
pub(super) const SOUTH_EAST: [u32; 4] = [SIZE / 2, SIZE / 2, SIZE, SIZE];

/// The texts of the labels drawn in `region`, picked where its text pixels are.
pub(super) fn texts_in(map: &SymbolMap, pixels: &[u8], region: [u32; 4]) -> Vec<String> {
    let mut texts: Vec<String> = count(pixels, TEXT, region)
        .iter()
        .step_by(7)
        .flat_map(|[x, y]| {
            map.map
                .query_rendered_symbols([f64::from(*x), f64::from(*y)], Some(&["labels"]))
        })
        .map(|hit| hit.text)
        .filter(|text| !text.is_empty())
        .collect();
    texts.sort();
    texts.dedup();
    texts
}

/// Left, top, advance and atlas size of each of `text`'s glyphs, as the atlas packs them.
fn shapes(glyphs: &crate::sdf::glyphs::Glyphs, text: &str) -> Vec<[f32; 5]> {
    text.chars()
        .map(|character| {
            let glyph = glyphs
                .stacks
                .iter()
                .flat_map(|stack| &stack.glyphs)
                .find(|glyph| glyph.id == u32::from(character))
                .expect("the range has the glyph");
            [
                glyph.left as f32 - 3.0,
                glyph.top as f32 + 3.0,
                glyph.advance as f32,
                (glyph.width + 6) as f32,
                (glyph.height + 6) as f32,
            ]
        })
        .collect()
}

/// The shapes the labels' atlas holds for `text` in the fixture font.
fn drawn_shapes(map: &SymbolMap, text: &str) -> Vec<[f32; 5]> {
    let world = &map.map.map_context.world;
    let atlas = world
        .tiles
        .tiles
        .values()
        .filter_map(|tile| {
            world
                .tiles
                .query::<&crate::sdf::SymbolLayersDataComponent>(tile.coords)
        })
        .flat_map(|symbols| &symbols.layers)
        .filter(|layer| !layer.buffer.buffer.indices.is_empty())
        .filter_map(|layer| layer.atlas.as_ref())
        .find(|atlas| {
            atlas
                .glyphs
                .get(FONT)
                .is_some_and(|glyphs| text.chars().all(|c| glyphs.contains_key(&u32::from(c))))
        })
        .expect("a drawn layer's atlas holds the text");
    text.chars()
        .map(|character| {
            let entry = &atlas.glyphs[FONT][&u32::from(character)];
            let [left, top, advance, _] = entry.metrics;
            [
                left,
                top,
                advance,
                entry.rect[2] as f32,
                entry.rect[3] as f32,
            ]
        })
        .collect()
}

/// The width in screen pixels `shapes` set at `size` cover, from the first glyph's left edge
/// to the last one's right, its bitmap's three-pixel border left out.
fn set_width(shapes: &[[f32; 5]], size: f32) -> f32 {
    let (Some(first), Some(last)) = (shapes.first(), shapes.last()) else {
        return 0.0;
    };
    let advances: f32 = shapes[..shapes.len() - 1]
        .iter()
        .map(|shape| shape[2])
        .sum();
    let right = advances + last[0] + 3.0 + last[3] - 6.0;
    (right - (first[0] + 3.0)) * size / 24.0
}

/// The width of the text pixels in `region`.
fn drawn_width(pixels: &[u8], region: [u32; 4]) -> f32 {
    let xs: Vec<u32> = count(pixels, TEXT, region)
        .iter()
        .map(|[x, _]| *x)
        .collect();
    match (xs.iter().min(), xs.iter().max()) {
        (Some(left), Some(right)) => (right - left + 1) as f32,
        _ => 0.0,
    }
}

#[tokio::test]
async fn geojson_labels_draw_with_the_font_and_sprite_they_ask_for_and_follow_set_data() {
    let mut map = SymbolMap::new(style(collection(vec![
        point(Some("Alps"), None, [-10.0, 6.0]),
        point(Some("Жук"), None, [10.0, 6.0]),
        point(None, Some("marker"), [10.0, -6.0]),
    ])))
    .await;
    let pixels = map.settle().await;
    assert_eq!(
        map.server.glyph_requests(),
        [
            "https://glyphs.test/Noto%20Sans%20Regular/0-255.pbf",
            "https://glyphs.test/Noto%20Sans%20Regular/1024-1279.pbf",
        ],
        "the label's own font, for its Latin and Cyrillic ranges"
    );
    let requested = map.server.requested();
    for sprite in [
        "https://sprites.test/sprite.json",
        "https://sprites.test/sprite.png",
    ] {
        assert!(
            requested.iter().any(|url| url == sprite),
            "{sprite}: {requested:?}"
        );
    }
    // Every glyph drawn is the served font's: the Cyrillic range exists nowhere else, and the
    // Latin glyphs are spaced as only the served range spaces them.
    use prost::Message as _;
    let served = crate::sdf::glyphs::Glyphs::decode(&super::latin()[..]).expect("served range");
    let bundled =
        crate::sdf::glyphs::Glyphs::decode(&include_bytes!("../../../../../data/0-255.pbf")[..])
            .expect("bundled range");
    assert_ne!(shapes(&served, "Alps"), shapes(&bundled, "Alps"));
    assert_eq!(drawn_shapes(&map, "Alps"), shapes(&served, "Alps"));
    let width = drawn_width(&pixels, NORTH_WEST);
    let (spaced, unspaced) = (
        set_width(&shapes(&served, "Alps"), 28.0),
        set_width(&shapes(&bundled, "Alps"), 28.0),
    );
    assert!(
        (width - spaced).abs() < 3.0 && (width - unspaced).abs() > 4.0,
        "'Alps' is {width} px wide: {spaced} px in the served spacing, {unspaced} px in the bundled"
    );
    assert_eq!(texts_in(&map, &pixels, NORTH_WEST), ["Alps"]);
    assert_eq!(texts_in(&map, &pixels, NORTH_EAST), ["Жук"]);
    let icon = count(&pixels, MARKER, SOUTH_EAST);
    assert!(
        icon.len() > 150,
        "the sprite's marker is drawn: {} pixels",
        icon.len()
    );
    assert!(count(&pixels, MARKER, SOUTH_WEST).is_empty());

    // New data moves the Cyrillic label, renames it, and moves the icon.
    map.map
        .map_context
        .set_geojson_data(
            "places",
            GeoJsonData::Inline(Arc::new(collection(vec![
                point(Some("Мир"), None, [-10.0, -6.0]),
                point(None, Some("marker"), [-10.0, 6.0]),
            ]))),
        )
        .expect("set data");
    let pixels = map.settle().await;
    assert!(
        texts_in(&map, &pixels, NORTH_EAST).is_empty(),
        "the old label is gone"
    );
    assert!(
        texts_in(&map, &pixels, NORTH_WEST).is_empty(),
        "Alps is gone"
    );
    assert_eq!(texts_in(&map, &pixels, SOUTH_WEST), ["Мир"]);
    assert!(
        count(&pixels, MARKER, SOUTH_EAST).is_empty(),
        "the icon left"
    );
    assert!(
        count(&pixels, MARKER, NORTH_WEST).len() > 150,
        "the icon moved"
    );
}
