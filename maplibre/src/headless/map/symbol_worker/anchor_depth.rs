//! A label is hidden only by ground standing in front of its anchor. The scene depth that
//! judges an anchor is read at the anchor's own place on the target, which on a display denser
//! than one device pixel per layout pixel is not the anchor's layout pixel.

mod navigation;
mod ridge;

use cgmath::Point2;

use super::{
    provided::{Answer, Shields, ROUTE_COLOURS},
    AssetServer, SymbolMap, FONT, GLYPHS, MARKER, SHIELD as SPRITE_SHIELD, SPRITE,
};
use crate::{coords::LatLon, render::projection::globe_camera_for_view, style::Style};

const WIDE: [u32; 2] = [1024, 512];
const SQUARE: [u32; 2] = [512, 512];
const RATIOS: [f64; 3] = [1.0, 2.0, 3.0];
/// The zoom at one device pixel per layout pixel; a denser display shows the same ground, at
/// the same device pixels, from a lower zoom.
const ZOOM: f64 = 3.0;
/// A zoom at which the globe covers even the wide target, so ground lies past every edge.
const CLOSE_ZOOM: f64 = 4.5;
/// Where markers stand, as fractions of the target: its middle and near each edge, where the
/// globe's surface lies farther from the eye than at the middle.
const PLACES: [[f64; 2]; 5] = [[0.5, 0.5], [0.1, 0.5], [0.9, 0.5], [0.5, 0.2], [0.5, 0.8]];
const PLACE_NAMES: [&str; 5] = ["middle", "left", "right", "top", "bottom"];
const KIND_NAMES: [&str; 3] = ["text", "icon", "inline image"];
/// Smallest opaque interiors at one device pixel per layout pixel: two 16-pixel `Ā` blocks, a
/// 16-pixel icon and a 20-pixel inline image, short of their antialiased rims.
const MINIMUM: [usize; 3] = [80, 150, 250];

/// What a set of markers draws: text from the font's solid `Ā` block in one colour, a sprite
/// icon and a provided inline image.
struct Look {
    text: &'static str,
    icon: &'static str,
    image: &'static str,
    /// The colours of the text, the icon and the inline image.
    colours: [[u8; 3]; 3],
}

const SEEN: Look = Look {
    text: "#0000ff",
    icon: "marker",
    image: "shield:A=1",
    colours: [[0, 0, 255], MARKER, ROUTE_COLOURS[0]],
};
/// Markers that must stay hidden, in colours of their own.
const HIDDEN: Look = Look {
    text: "#ff0000",
    icon: "shield",
    image: "shield:B=1",
    colours: [[255, 0, 0], SPRITE_SHIELD, ROUTE_COLOURS[1]],
};

/// Where a place's three markers stand around its anchor, so each kind's pixels can be counted
/// apart and neighbouring places' markers keep apart at three device pixels per layout pixel.
struct Arrangement {
    anchor: &'static str,
    /// Text and inline image offsets in ems, icon offset in pixels.
    text: [f64; 2],
    icon: [f64; 2],
    inline: [f64; 2],
}

/// Text above the anchor, the icon on it and the inline image below.
const COLUMN: Arrangement = Arrangement {
    anchor: "center",
    text: [0.0, -1.3],
    icon: [0.0, 0.0],
    inline: [0.0, 1.3],
};
/// Text left of the anchor, the icon on it and the inline image right of it.
const ROW: Arrangement = Arrangement {
    anchor: "center",
    text: [-1.8, 0.0],
    icon: [0.0, 0.0],
    inline: [1.6, 0.0],
};
/// A column standing left of an anchor past the right edge, back on the target.
const PAST: Arrangement = Arrangement {
    anchor: "right",
    text: [-2.5, -1.3],
    icon: [-40.0, 0.0],
    inline: [-2.5, 1.3],
};
/// Places by a side stand in a column, places on the vertical middle line in a row.
const SET_OF_PLACE: [&str; 5] = ["row", "column", "column", "row", "row"];

/// One layer per kind of marker for the points whose `set` property is `set`.
fn marker_layers(set: &str, arrangement: &Arrangement, look: &Look) -> Vec<serde_json::Value> {
    let placement = |mut layout: serde_json::Value| {
        for (key, value) in [
            ("text-font", serde_json::json!([FONT])),
            ("text-size", serde_json::json!(16)),
            ("text-allow-overlap", serde_json::json!(true)),
            ("text-ignore-placement", serde_json::json!(true)),
            ("icon-allow-overlap", serde_json::json!(true)),
            ("icon-ignore-placement", serde_json::json!(true)),
        ] {
            layout[key] = value;
        }
        layout
    };
    let Arrangement {
        anchor,
        text,
        icon,
        inline,
    } = arrangement;
    let filter = serde_json::json!(["==", ["get", "set"], set]);
    vec![
        serde_json::json!({"id":format!("{set} text"),"type":"symbol","source":"places",
            "filter":filter,
            "layout":placement(serde_json::json!({"text-field":"ĀĀ","text-anchor":anchor,
                "text-offset":text})),
            "paint":{"text-color":look.text}}),
        serde_json::json!({"id":format!("{set} icon"),"type":"symbol","source":"places",
            "filter":filter,
            "layout":placement(serde_json::json!({"icon-image":look.icon,"icon-anchor":anchor,
                "icon-offset":icon}))}),
        serde_json::json!({"id":format!("{set} inline"),"type":"symbol","source":"places",
            "filter":filter,
            "layout":placement(serde_json::json!({
                "text-field":["format",["image",look.image],{}],"text-anchor":anchor,
                "text-offset":inline}))}),
    ]
}

/// A globe filling the target, with markers at `places`.
fn globe_style(zoom: f64, places: serde_json::Value) -> Style {
    let mut layers = vec![serde_json::json!({"id":"background","type":"background",
        "paint":{"background-color":"#223344"}})];
    for (set, arrangement) in [("column", &COLUMN), ("row", &ROW), ("past", &PAST)] {
        layers.extend(marker_layers(set, arrangement, &SEEN));
    }
    serde_json::from_value(serde_json::json!({
        "version":8,"center":[10.0,20.0],"zoom":zoom,"glyphs":GLYPHS,"sprite":SPRITE,
        "projection":{"type":"vertical-perspective"},
        "sources":{"places":{"type":"geojson","data":places}},
        "layers":layers
    }))
    .expect("style")
}

/// Points of the globe under the given fractions of a `size` target, as `zoom` shows it at
/// one device pixel per layout pixel.
async fn points_under(size: [u32; 2], zoom: f64, fractions: &[[f64; 2]]) -> Vec<LatLon> {
    let probe = SymbolMap::serving_sized(
        globe_style(zoom, features(&[])),
        AssetServer::default(),
        size,
    )
    .await;
    let camera = globe_camera_for_view(probe.map.view_state()).expect("globe camera");
    fractions
        .iter()
        .map(|[x, y]| {
            camera
                .screen_point_to_location(Point2::new(
                    x * f64::from(size[0]),
                    y * f64::from(size[1]),
                ))
                .unwrap_or_else(|| panic!("{x}, {y} of the target lies on the globe"))
        })
        .collect()
}

/// Point features, each with the `set` of markers it shows.
fn features(points: &[(LatLon, &str)]) -> serde_json::Value {
    let features: Vec<_> = points
        .iter()
        .map(|(point, set)| {
            serde_json::json!({"type":"Feature","properties":{"set":set},
                "geometry":{"type":"Point","coordinates":[point.longitude,point.latitude]}})
        })
        .collect();
    serde_json::json!({"type":"FeatureCollection","features":features})
}

/// The map drawing `style` into a `size` target at `ratio` device pixels per layout pixel,
/// with the ridges' DEM for a style that asks for terrain.
async fn map_at(style: Style, size: [u32; 2], ratio: f64) -> SymbolMap {
    let mut map = SymbolMap::serving_sized(style, ridge::server(), size).await;
    map.map
        .image_providers()
        .expect("registry")
        .register("shield", Shields::new(Answer::PerRoute));
    map.map.set_pixel_ratio(ratio);
    map
}

/// The pixels of each kind of marker in `colours` nearest each of `anchors`, in device pixels.
fn counts_near(
    pixels: &[u8],
    width: u32,
    anchors: &[[f64; 2]],
    colours: &[[u8; 3]; 3],
) -> Vec<[usize; 3]> {
    let mut counts = vec![[0; 3]; anchors.len()];
    for (index, pixel) in pixels.chunks_exact(4).enumerate() {
        let Some(kind) = colours.iter().position(|colour| {
            pixel
                .iter()
                .zip(colour)
                .all(|(have, want)| have.abs_diff(*want) < 60)
        }) else {
            continue;
        };
        let (x, y) = ((index as u32 % width) as f64, (index as u32 / width) as f64);
        let nearest = (0..anchors.len()).min_by(|a, b| {
            let distance = |i: &usize| (anchors[*i][0] - x).hypot(anchors[*i][1] - y);
            distance(a).total_cmp(&distance(b))
        });
        if let Some(nearest) = nearest {
            counts[nearest][kind] += 1;
        }
    }
    counts
}

/// The device pixels of `fractions` of a `size` target.
fn device_pixels(size: [u32; 2], fractions: &[[f64; 2]]) -> Vec<[f64; 2]> {
    fractions
        .iter()
        .map(|[x, y]| [x * f64::from(size[0]), y * f64::from(size[1])])
        .collect()
}

#[tokio::test]
async fn markers_across_a_globe_stay_visible_at_every_pixel_ratio() {
    let mut failures = Vec::new();
    for size in [WIDE, SQUARE] {
        let points = points_under(size, ZOOM, &PLACES).await;
        let places = features(&points.into_iter().zip(SET_OF_PLACE).collect::<Vec<_>>());
        let anchors = device_pixels(size, &PLACES);
        let mut reference = Vec::new();
        for ratio in RATIOS {
            let style = globe_style(ZOOM - ratio.log2(), places.clone());
            let pixels = map_at(style, size, ratio).await.settle().await;
            let counts = counts_near(&pixels, size[0], &anchors, &SEEN.colours);
            if ratio == 1.0 {
                reference = counts.clone();
            }
            for (place, (count, base)) in counts.iter().zip(&reference).enumerate() {
                for (kind, name) in KIND_NAMES.iter().enumerate() {
                    let area = ratio * ratio;
                    let enough = count[kind] as f64 >= MINIMUM[kind] as f64 * area;
                    let scaled = count[kind] as f64 / (base[kind] as f64 * area);
                    if !enough || !(0.7..1.4).contains(&scaled) {
                        failures.push(format!(
                            "{size:?} at {ratio}x: the {name} at the {} shows {} pixels, {} at 1x",
                            PLACE_NAMES[place], count[kind], base[kind]
                        ));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn a_label_whose_anchor_lies_past_the_edge_is_not_judged_by_the_edges_ground() {
    let past = [[1.03, 0.3], [1.03, 0.7]];
    let mut failures = Vec::new();
    for size in [WIDE, SQUARE] {
        let points = points_under(size, CLOSE_ZOOM, &past).await;
        let places = features(
            &points
                .into_iter()
                .map(|point| (point, "past"))
                .collect::<Vec<_>>(),
        );
        // The markers stand left of their anchors.
        let anchors: Vec<[f64; 2]> = device_pixels(size, &past)
            .into_iter()
            .map(|[x, y]| [x - 60.0, y])
            .collect();
        for ratio in RATIOS {
            let style = globe_style(CLOSE_ZOOM - ratio.log2(), places.clone());
            let pixels = map_at(style, size, ratio).await.settle().await;
            let counts = counts_near(&pixels, size[0], &anchors, &SEEN.colours);
            for (place, count) in counts.iter().enumerate() {
                for (kind, name) in KIND_NAMES.iter().enumerate() {
                    if count[kind] < MINIMUM[kind] {
                        failures.push(format!(
                            "{size:?} at {ratio}x: the {name} of anchor {:?} shows {} pixels",
                            past[place], count[kind]
                        ));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
