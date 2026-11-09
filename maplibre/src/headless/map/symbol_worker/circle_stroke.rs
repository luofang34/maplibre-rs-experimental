//! A circle stroke colour that varies by feature reaches the screen: each circle's ring takes
//! the colour its feature names, through the worker's tessellation and the circle shader.

use super::{SymbolMap, FONT, GLYPHS, SIZE, SPRITE};
use crate::style::Style;

/// Pixels per degree of longitude at zoom 2 in a 512-pixel view.
const PIXELS_PER_DEGREE: f64 = 512.0 * 4.0 / 360.0;

fn style() -> Style {
    let point = |edge: &str, longitude: f64| {
        serde_json::json!({"type":"Feature","properties":{"edge":edge,"name":"x"},
            "geometry":{"type":"Point","coordinates":[longitude,0.0]}})
    };
    serde_json::from_value(serde_json::json!({
        "version":8,"center":[0,0],"zoom":2,
        "glyphs":GLYPHS,"sprite":SPRITE,
        "sources":{"spots":{"type":"geojson","data":{"type":"FeatureCollection",
            "features":[point("#ff0000", -10.0), point("#0000ff", 10.0)]}}},
        "layers":[
            {"id":"background","type":"background","paint":{"background-color":"#000000"}},
            {"id":"dots","type":"circle","source":"spots",
                "paint":{"circle-radius":10,"circle-color":"#00ff00","circle-stroke-width":6,
                    "circle-stroke-color":["get","edge"]}},
            // The harness settles once a tile's labels reach the GPU; this one sits below the
            // circles, clear of the rings sampled.
            {"id":"names","type":"symbol","source":"spots",
                "layout":{"text-field":["get","name"],"text-font":[FONT],"text-size":10,
                    "text-anchor":"top","text-offset":[0,3],"text-allow-overlap":true},
                "paint":{"text-color":"#ffffff"}}
        ]
    }))
    .expect("style")
}

fn pixel(pixels: &[u8], x: f64, y: f64) -> [u8; 3] {
    let index = ((y as u32 * SIZE + x as u32) * 4) as usize;
    [pixels[index], pixels[index + 1], pixels[index + 2]]
}

#[tokio::test]
async fn each_circle_strokes_with_the_colour_its_feature_names() {
    let style = style();
    style
        .validate()
        .expect("a per-feature stroke colour validates");
    let mut map = SymbolMap::new(style).await;
    let pixels = map.settle().await;

    let middle = f64::from(SIZE) / 2.0;
    for (longitude, stroke) in [(-10.0, [255, 0, 0]), (10.0, [0, 0, 255])] {
        let center = middle + longitude * PIXELS_PER_DEGREE;
        assert_eq!(
            pixel(&pixels, center, middle),
            [0, 255, 0],
            "the fill at {longitude}"
        );
        for ring in [center - 13.0, center + 13.0] {
            assert_eq!(
                pixel(&pixels, ring, middle),
                stroke,
                "the ring of the circle at {longitude}"
            );
        }
    }
}
