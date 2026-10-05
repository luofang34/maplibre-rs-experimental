//! A road carrying several routes shows one shield per route in a row along it, as OSM
//! Americana lays them out with `viewport-glyph`: in route order, each upright, the row one
//! label that collides as a whole and by every shield's own size.

use super::*;
use crate::{
    headless::map::symbol_worker::{FONT, GLYPHS},
    sdf::query::PlacedSymbols,
};

/// Every tile: a road across its middle carrying routes `A=1`, `B=2` and `C=3`.
fn three_route_tile() -> Vec<u8> {
    let string = |value: &str| tile::Value {
        string_value: Some(value.to_owned()),
        ..Default::default()
    };
    let mut keys = Vec::new();
    let mut values = Vec::new();
    let mut tags = Vec::new();
    for (n, network) in ["A", "B", "C"].iter().enumerate() {
        for (field, value) in [("network", *network), ("ref", &format!("{}", n + 1)[..])] {
            tags.extend([keys.len() as u32, values.len() as u32]);
            keys.push(format!("route_{}_{field}", n + 1));
            values.push(string(value));
        }
    }
    geozero::mvt::Tile {
        layers: vec![tile::Layer {
            version: 2,
            name: "roads".into(),
            extent: Some(4096),
            keys,
            values,
            features: vec![tile::Feature {
                r#type: Some(tile::GeomType::Linestring as i32),
                tags,
                geometry: vec![9, 0, 4096, 10, 8192, 0],
                ..Default::default()
            }],
        }],
    }
    .encode_to_vec()
}

fn groups_style(globe: bool, terrain: bool) -> Style {
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
    let mut style = serde_json::json!({
        "version":8,"center":[0.01,0.010986328],"zoom":14,"pitch":30,"bearing":20,
        "glyphs":GLYPHS,"sprite":SPRITE,
        "sources":{"streets":{"type":"vector","tiles":["https://tiles.test/{z}/{x}/{y}.pbf"],
            "maxzoom":14},
            "dem":{"type":"raster-dem","tiles":["https://dem.test/{z}/{x}/{y}.png"],
                "encoding":"terrarium","maxzoom":12,"tileSize":256}},
        "layers":[
            {"id":"background","type":"background","paint":{"background-color":"#223344"}},
            {"id":"road","type":"line","source":"streets","source-layer":"roads",
                "paint":{"line-color":"#808080","line-width":3}},
            {"id":"shields","type":"symbol","source":"streets","source-layer":"roads",
                "layout":{"symbol-placement":"line","symbol-spacing":300,
                    "text-rotation-alignment":"viewport-glyph","text-pitch-alignment":"viewport",
                    "text-max-angle":180,"text-letter-spacing":0.7,
                    "text-field":["format", route(1), route(2), route(3), route(4)],
                    "text-font":[FONT],"text-size":10},
                "paint":{"text-color":"#ff0000"}}
        ]
    });
    if globe {
        style["projection"] = serde_json::json!({"type":"vertical-perspective"});
    }
    if terrain {
        style["terrain"] = serde_json::json!({"source":"dem","exaggeration":1});
    }
    serde_json::from_value(style).expect("style")
}

#[tokio::test]
async fn a_road_of_three_routes_shows_their_shields_in_a_row_that_collides_as_one_label() {
    for (globe, terrain) in [(false, false), (true, false), (false, true)] {
        let case = match (globe, terrain) {
            (true, _) => "globe",
            (_, true) => "terrain",
            _ => "mercator",
        };
        let shields = Shields::new(Answer::PerRoute);
        let server = AssetServer::default();
        server.serve("https://tiles.test/", three_route_tile());
        server.serve(
            "https://dem.test/",
            crate::headless::map::symbol_worker::mvt::dem_tile(),
        );
        let mut map = SymbolMap::serving(groups_style(globe, terrain), server).await;
        map.map
            .image_providers()
            .expect("registry")
            .register("shield", shields.clone());
        let pixels = map.settle().await;
        assert_eq!(
            shown(&pixels, [255, 0, 0]),
            0,
            "{case}: no request name drawn as text"
        );
        let placed = map
            .map
            .map_context
            .world
            .resources
            .get::<PlacedSymbols>()
            .expect("placed symbols");
        // The label nearest the middle of the view.
        let middle = f64::from(SIZE) / 2.0;
        let label = placed
            .0
            .iter()
            .filter(|symbol| symbol.layer == "shields" && !symbol.glyph_boxes.is_empty())
            .min_by(|a, b| {
                let off = |boxes: &Vec<[f64; 4]>| {
                    let [l, t, r, bottom] = boxes[boxes.len() / 2];
                    ((l + r) / 2.0 - middle).hypot((t + bottom) / 2.0 - middle)
                };
                off(&a.glyph_boxes).total_cmp(&off(&b.glyph_boxes))
            })
            .expect("a row of shields placed glyph by glyph");
        assert_eq!(label.glyph_boxes.len(), 3, "{case}: one box per shield");
        // Each shield's own pixels near the label lie inside its box, in route order.
        let mut centres = Vec::new();
        for (index, colour) in ROUTE_COLOURS.iter().enumerate() {
            let [l, t, r, b] = label.glyph_boxes[index];
            let inside: Vec<[u32; 2]> = count(&pixels, *colour, WHOLE)
                .into_iter()
                .filter(|[x, y]| {
                    let (x, y) = (f64::from(*x) + 0.5, f64::from(*y) + 0.5);
                    x >= l - 2.0 && x <= r + 2.0 && y >= t - 2.0 && y <= b + 2.0
                })
                .collect();
            assert!(
                inside.len() as f32 > SIDE * SIDE * 0.8,
                "{case}: shield {index} lies within its collision box: {} pixels",
                inside.len()
            );
            let width = r - l;
            // The text is half the shield's size, so only the shield's own size covers it.
            assert!(
                width >= f64::from(SIDE),
                "{case}: shield {index}'s box is as wide as the shield: {width}"
            );
            centres.push((l + r) / 2.0);
        }
        assert!(
            centres.windows(2).all(|pair| pair[0] < pair[1]),
            "{case}: the shields read in route order: {centres:?}"
        );
        // The shields stand apart, not on top of each other.
        for pair in centres.windows(2) {
            assert!(pair[1] - pair[0] > f64::from(SIDE), "{case}: {centres:?}");
        }
        // One label: a hit on any of its shields finds the one feature.
        let boxes = label.glyph_boxes.clone();
        let hits: Vec<_> = boxes
            .iter()
            .map(|[l, t, r, b]| {
                map.map
                    .query_rendered_symbols([(l + r) / 2.0, (t + b) / 2.0], Some(&["shields"]))
            })
            .collect();
        for hit in &hits {
            assert_eq!(
                hit.len(),
                1,
                "{case}: one label under each shield: {hits:?}"
            );
        }
        assert!(
            hits.iter()
                .all(|hit| hit[0].properties == hits[0][0].properties),
            "{case}: the road's one label under every shield"
        );
        assert_eq!(
            shields.calls(),
            3,
            "{case}: each route's shield is made once"
        );
        assert_rows_apart(&map, case);
        // Zoomed past the source's last zoom, the tiles are magnified and their labels laid
        // out again; repeated rows still keep apart.
        map.map
            .view_state_mut()
            .zoom_to(crate::coords::Zoom::new(14.7));
        map.settle().await;
        assert_rows_apart(&map, &format!("{case} at zoom 14.7"));
        // Zoomed out past a whole level, parent tiles replace their children; while both are
        // held, the rows of the one road never stack on one another.
        map.map
            .view_state_mut()
            .zoom_to(crate::coords::Zoom::new(13.2));
        for frame in 0..90 {
            map.frame().await;
            assert_rows_do_not_overlap(&map, &format!("{case} zooming out, frame {frame}"));
        }
        map.settle().await;
        assert_rows_apart(&map, &format!("{case} at zoom 13.2"));
        assert_eq!(
            shields.calls(),
            3,
            "{case}: no shield is made again for the zoom"
        );
    }
}

/// No two rows of shields drawn for the road overlap.
fn assert_rows_do_not_overlap(map: &SymbolMap, case: &str) {
    let placed = map
        .map
        .map_context
        .world
        .resources
        .get::<PlacedSymbols>()
        .expect("placed symbols");
    let rows: Vec<&Vec<[f64; 4]>> = placed
        .0
        .iter()
        .filter(|symbol| symbol.layer == "shields")
        .map(|symbol| &symbol.glyph_boxes)
        .collect();
    let overlap =
        |a: &[f64; 4], b: &[f64; 4]| a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3];
    for (index, row) in rows.iter().enumerate() {
        for other in &rows[index + 1..] {
            assert!(
                !row.iter().any(|a| other.iter().any(|b| overlap(a, b))),
                "{case}: two rows overlap: {row:?} {other:?}"
            );
        }
    }
}

/// Rows of shields drawn for the road keep their spacing and never overlap one another.
fn assert_rows_apart(map: &SymbolMap, case: &str) {
    let placed = map
        .map
        .map_context
        .world
        .resources
        .get::<PlacedSymbols>()
        .expect("placed symbols");
    let rows: Vec<&Vec<[f64; 4]>> = placed
        .0
        .iter()
        .filter(|symbol| symbol.layer == "shields")
        .map(|symbol| &symbol.glyph_boxes)
        .collect();
    assert!(
        rows.len() >= 2,
        "{case}: the road repeats its row: {}",
        rows.len()
    );
    let overlap =
        |a: &[f64; 4], b: &[f64; 4]| a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3];
    for (index, row) in rows.iter().enumerate() {
        for other in &rows[index + 1..] {
            assert!(
                !row.iter().any(|a| other.iter().any(|b| overlap(a, b))),
                "{case}: two rows overlap: {row:?} {other:?}"
            );
            let centre = |row: &[[f64; 4]]| {
                let [l, t, r, b] = row[row.len() / 2];
                [(l + r) / 2.0, (t + b) / 2.0]
            };
            let ([x, y], [ox, oy]) = (centre(row), centre(other));
            assert!(
                (x - ox).hypot(y - oy) > 100.0,
                "{case}: rows {:.0} px apart",
                (x - ox).hypot(y - oy)
            );
        }
    }
}

/// Every tile: two roads `gap` tile units apart, the secondary road first, each carrying one
/// route.
fn two_class_tile(gap: u32) -> Vec<u8> {
    let string = |value: &str| tile::Value {
        string_value: Some(value.to_owned()),
        ..Default::default()
    };
    let keys = ["class", "route_1_network", "route_1_ref"]
        .map(String::from)
        .to_vec();
    let mut values = Vec::new();
    let mut features = Vec::new();
    for (class, network, y) in [("secondary", "C", 2048), ("motorway", "A", 2048 + gap)] {
        let base = values.len() as u32;
        values.extend([string(class), string(network), string("1")]);
        features.push(tile::Feature {
            r#type: Some(tile::GeomType::Linestring as i32),
            tags: vec![0, base, 1, base + 1, 2, base + 2],
            geometry: vec![9, 0, y * 2, 10, 8192, 0],
            ..Default::default()
        });
    }
    geozero::mvt::Tile {
        layers: vec![tile::Layer {
            version: 2,
            name: "roads".into(),
            extent: Some(4096),
            keys,
            values,
            features,
        }],
    }
    .encode_to_vec()
}

#[tokio::test]
async fn the_more_important_road_keeps_its_shields_where_two_rows_would_collide() {
    let [motorway, _, secondary] = ROUTE_COLOURS;
    for sorted in [true, false] {
        let mut style = groups_style(false, false);
        if sorted {
            style
                .set_layout_property(
                    "shields",
                    "symbol-sort-key",
                    serde_json::json!([
                        "match",
                        ["get", "class"],
                        "motorway",
                        0,
                        "trunk",
                        1,
                        "primary",
                        2,
                        "secondary",
                        3,
                        "tertiary",
                        4,
                        5
                    ]),
                )
                .expect("sort key");
        }
        let shields = Shields::new(Answer::PerRoute);
        let server = AssetServer::default();
        // A few pixels apart, so the two roads' rows collide.
        server.serve("https://tiles.test/", two_class_tile(40));
        let mut map = SymbolMap::serving(style, server).await;
        map.map
            .image_providers()
            .expect("registry")
            .register("shield", shields);
        let pixels = map.settle().await;
        let (kept, hidden) = if sorted {
            (motorway, secondary)
        } else {
            // Without a sort key the road first in the tile is placed first.
            (secondary, motorway)
        };
        assert!(
            shown(&pixels, kept) > 300,
            "sorted {sorted}: the first road's shields show"
        );
        assert_eq!(
            shown(&pixels, hidden),
            0,
            "sorted {sorted}: the other road's rows yield"
        );
    }
}
