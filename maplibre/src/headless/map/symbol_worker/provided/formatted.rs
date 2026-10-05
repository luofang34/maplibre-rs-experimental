//! Shields named by `image` inside a `format` text field, chosen by a `case` for each route of
//! a road as OSM Americana lays them out: each is drawn as an image in the line of text, never
//! as the text of the name it was requested by, and appears once it arrives.

use std::sync::atomic::Ordering;

use super::*;
use crate::headless::map::symbol_worker::{FONT, GLYPHS};

/// The text colour; a request name drawn as text would show in it.
const TEXT: [u8; 3] = [255, 0, 0];

/// A spot carrying two routes, labelled by a `format` of one `case`-chosen image per route.
fn routes_style() -> Style {
    let route = |n: u8| {
        serde_json::json!([
            "case",
            ["has", format!("route_{n}_network")],
            [
                "image",
                [
                    "concat",
                    "shield:",
                    ["get", format!("route_{n}_network")],
                    "=",
                    ["get", format!("route_{n}_ref")]
                ]
            ],
            ["literal", ""]
        ])
    };
    serde_json::from_value(serde_json::json!({
        "version":8,"center":[0.01,0.010986328],"zoom":14,"glyphs":GLYPHS,"sprite":SPRITE,
        "sources":{"spot":{"type":"geojson","data":{"type":"Feature",
            "properties":{"route_1_network":"BAB","route_1_ref":"A 115",
                "route_2_network":"e-road","route_2_ref":"E 51"},
            "geometry":{"type":"Point","coordinates":[0.01,0.010986328]}}}},
        "layers":[
            {"id":"background","type":"background","paint":{"background-color":"#223344"}},
            {"id":"shields","type":"symbol","source":"spot",
                "layout":{"text-field":["format", route(1), route(2), route(3)],
                    "text-font":[FONT],"text-size":16,"text-letter-spacing":0.7,
                    "text-allow-overlap":true},
                "paint":{"text-color":"#ff0000"}}
        ]
    }))
    .expect("style")
}

#[tokio::test]
async fn shields_a_case_chooses_inside_format_are_drawn_as_images_never_as_their_names() {
    let (shields, gate) = Shields::held(Answer::Shield(SHIELD));
    let mut map = SymbolMap::serving(routes_style(), AssetServer::default()).await;
    map.map
        .image_providers()
        .expect("registry")
        .register("shield", shields.clone());
    // While the shields are made the label has nothing to draw: no name shows as text.
    for _ in 0..40 {
        map.frame().await;
        let pixels = map.read();
        assert_eq!(shown(&pixels, TEXT), 0, "a request name is drawn as text");
        assert_eq!(shown(&pixels, SHIELD), 0);
    }
    let mut requested: Vec<String> = shields
        .requests
        .lock()
        .expect("requests")
        .iter()
        .map(|request| request.id.clone())
        .collect();
    requested.sort();
    assert_eq!(
        requested,
        ["BAB=A 115", "e-road=E 51"],
        "every route's shield is asked for, the third case naming none"
    );
    gate.add_permits(64);
    // Once they arrive the label is laid out again with both, without the map moving.
    let pixels = map.settle().await;
    assert_eq!(shown(&pixels, TEXT), 0, "a request name is drawn as text");
    let shield = count(&pixels, SHIELD, WHOLE);
    let side = (SIDE as usize).pow(2);
    assert!(
        shield.len() > side * 3 / 2,
        "both routes' shields are drawn: {} pixels",
        shield.len()
    );
    // Side by side along the line of text, not on top of each other.
    let xs = shield.iter().map(|[x, _]| *x);
    let width = xs.clone().max().expect("pixels") - xs.min().expect("pixels") + 1;
    assert!(
        width as f32 > SIDE * 2.0,
        "the shields stand {width} px across"
    );
    assert_eq!(shields.calls(), 2, "each shield is made once");
    assert_eq!(shields.during_frames.load(Ordering::SeqCst), 0);
}
