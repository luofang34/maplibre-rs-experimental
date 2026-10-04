//! GeoJSON labels, whose layers name no source layer, get the font and sprite they ask for.

use std::sync::Arc;

use super::{count, SymbolMap, FONT, GLYPHS, MARKER, SIZE, SPRITE};
use crate::style::{source::GeoJsonData, Style};

const TEXT: [u8; 3] = [255, 0, 0];

fn point(name: Option<&str>, icon: Option<&str>, at: [f64; 2]) -> serde_json::Value {
    serde_json::json!({"type":"Feature",
        "properties":{"name":name,"icon":icon},
        "geometry":{"type":"Point","coordinates":at}})
}

fn collection(features: Vec<serde_json::Value>) -> serde_json::Value {
    serde_json::json!({"type":"FeatureCollection","features":features})
}

fn style(data: serde_json::Value) -> Style {
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
const NORTH_WEST: [u32; 4] = [0, 0, SIZE / 2, SIZE / 2];
const NORTH_EAST: [u32; 4] = [SIZE / 2, 0, SIZE, SIZE / 2];
const SOUTH_WEST: [u32; 4] = [0, SIZE / 2, SIZE / 2, SIZE];
const SOUTH_EAST: [u32; 4] = [SIZE / 2, SIZE / 2, SIZE, SIZE];

/// The texts of the labels drawn in `region`, picked where its text pixels are.
fn texts_in(map: &SymbolMap, pixels: &[u8], region: [u32; 4]) -> Vec<String> {
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
    // Every glyph is the served font's: the Cyrillic range exists nowhere else, and the Latin
    // one was served, so the bundled range is never fallen back on.
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
