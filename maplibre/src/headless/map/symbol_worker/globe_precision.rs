//! A street name in a close view of the vertical-perspective globe keeps its glyphs square to
//! one straight street and evenly spaced: no glyph turns or slides on its own because the
//! globe's projection lost precision at metre scale.

use geozero::mvt::{tile, Message as _};

use super::{AssetServer, SymbolMap, FONT, GLYPHS, SIZE, SPRITE};
use crate::style::Style;

/// Where the DJI survey debrief flies, where the trouble was seen.
const CENTRE: [f64; 2] = [-74.458983, 40.545186];

/// The view centre in tile units of its zoom-14 tile, (4803, 6170).
const CENTRE_IN_TILE: [f64; 2] = [1183.545, 494.130];

/// Every tile: a straight street through the view centre, 30 degrees below east in tile
/// units, named with vertical strokes whose lean and centres measure each glyph.
fn street_tile() -> Vec<u8> {
    let zigzag = |value: i32| ((value << 1) ^ (value >> 31)) as u32;
    let (dx, dy) = (30_f64.to_radians().cos(), 30_f64.to_radians().sin());
    let end = |sign: f64| {
        [
            (CENTRE_IN_TILE[0] + sign * 900.0 * dx).round() as i32,
            (CENTRE_IN_TILE[1] + sign * 900.0 * dy).round() as i32,
        ]
    };
    let (from, to) = (end(-1.0), end(1.0));
    geozero::mvt::Tile {
        layers: vec![tile::Layer {
            version: 2,
            name: "roads".into(),
            extent: Some(4096),
            keys: vec!["name".into()],
            values: vec![tile::Value {
                string_value: Some("IIIIIIIIIIIIIIII".into()),
                ..Default::default()
            }],
            features: vec![tile::Feature {
                r#type: Some(tile::GeomType::Linestring as i32),
                tags: vec![0, 0],
                geometry: vec![
                    9,
                    zigzag(from[0]),
                    zigzag(from[1]),
                    10,
                    zigzag(to[0] - from[0]),
                    zigzag(to[1] - from[1]),
                ],
                ..Default::default()
            }],
        }],
    }
    .encode_to_vec()
}

fn style(zoom: f64, terrain: bool) -> Style {
    let mut style = serde_json::json!({
        "version":8,"center":CENTRE,"zoom":zoom,"pitch":0,
        "projection":{"type":"vertical-perspective"},
        "glyphs":GLYPHS,"sprite":SPRITE,
        "sources":{"streets":{"type":"vector","tiles":["https://tiles.test/{z}/{x}/{y}.pbf"],"maxzoom":14},
            "dem":{"type":"raster-dem","tiles":["https://dem.test/{z}/{x}/{y}.png"],
                "encoding":"terrarium","tileSize":256,"maxzoom":12}},
        "layers":[
            {"id":"background","type":"background","paint":{"background-color":"#223344"}},
            {"id":"road","type":"line","source":"streets","source-layer":"roads",
                "paint":{"line-color":"#808080","line-width":2}},
            {"id":"road-name","type":"symbol","source":"streets","source-layer":"roads",
                "layout":{"symbol-placement":"line-center",
                    "text-field":["get","name"],"text-font":[FONT],"text-size":20,
                    "text-rotation-alignment":"map","text-pitch-alignment":"viewport",
                    "text-keep-upright":true,"text-allow-overlap":true},
                "paint":{"text-color":"#ff0000"}}
        ]
    });
    if terrain {
        style["terrain"] = serde_json::json!({"source":"dem","exaggeration":1});
    }
    serde_json::from_value(style).expect("style")
}

/// One drawn glyph: its centre and the angle of its long axis, in screen pixels and radians.
#[derive(Clone, Copy, Debug)]
struct Stroke {
    centre: [f64; 2],
    axis: f64,
    pixels: usize,
}

/// The connected red shapes in the frame, each weighted by how red its pixels are.
fn strokes(pixels: &[u8]) -> Vec<Stroke> {
    let side = SIZE as usize;
    let weight = |index: usize| {
        let pixel = &pixels[index * 4..index * 4 + 3];
        (f64::from(pixel[0]) - f64::from(pixel[1].max(pixel[2])) - 40.0).max(0.0)
    };
    let mut seen = vec![false; side * side];
    let mut found = Vec::new();
    for start in 0..side * side {
        if seen[start] || weight(start) <= 0.0 {
            continue;
        }
        let mut stack = vec![start];
        seen[start] = true;
        let mut members = Vec::new();
        while let Some(index) = stack.pop() {
            members.push(index);
            let (x, y) = (index % side, index / side);
            for (nx, ny) in [
                (x.wrapping_sub(1), y),
                (x + 1, y),
                (x, y.wrapping_sub(1)),
                (x, y + 1),
            ] {
                if nx < side && ny < side {
                    let next = ny * side + nx;
                    if !seen[next] && weight(next) > 0.0 {
                        seen[next] = true;
                        stack.push(next);
                    }
                }
            }
        }
        let total: f64 = members.iter().map(|index| weight(*index)).sum();
        let at = |index: usize| [(index % side) as f64 + 0.5, (index / side) as f64 + 0.5];
        let mean = members.iter().fold([0.0; 2], |sum, index| {
            let [x, y] = at(*index);
            [sum[0] + x * weight(*index), sum[1] + y * weight(*index)]
        });
        let centre = [mean[0] / total, mean[1] / total];
        let (mut xx, mut yy, mut xy) = (0.0, 0.0, 0.0);
        for index in &members {
            let [x, y] = at(*index);
            let (dx, dy, w) = (x - centre[0], y - centre[1], weight(*index));
            xx += w * dx * dx;
            yy += w * dy * dy;
            xy += w * dx * dy;
        }
        found.push(Stroke {
            centre,
            axis: 0.5 * (2.0 * xy).atan2(xx - yy),
            pixels: members.len(),
        });
    }
    found
}

/// How a label's strokes sit: the worst lean of a stroke from square to the line through
/// their centres in degrees, the worst distance of a centre from that line, and the worst
/// departure of a step between neighbours from their mean step, both in pixels.
fn measure(strokes: &[Stroke]) -> (f64, f64, f64) {
    let n = strokes.len() as f64;
    let mean = strokes.iter().fold([0.0; 2], |sum, stroke| {
        [sum[0] + stroke.centre[0] / n, sum[1] + stroke.centre[1] / n]
    });
    let (mut xx, mut yy, mut xy) = (0.0, 0.0, 0.0);
    for stroke in strokes {
        let (dx, dy) = (stroke.centre[0] - mean[0], stroke.centre[1] - mean[1]);
        xx += dx * dx;
        yy += dy * dy;
        xy += dx * dy;
    }
    let along = 0.5 * (2.0 * xy).atan2(xx - yy);
    let (ux, uy) = (along.cos(), along.sin());
    let lean = strokes
        .iter()
        .map(|stroke| {
            let difference = (stroke.axis - along).rem_euclid(std::f64::consts::PI);
            (difference - std::f64::consts::FRAC_PI_2)
                .abs()
                .to_degrees()
        })
        .fold(0.0, f64::max);
    let off = strokes
        .iter()
        .map(|stroke| ((stroke.centre[0] - mean[0]) * uy - (stroke.centre[1] - mean[1]) * ux).abs())
        .fold(0.0, f64::max);
    let mut along_line: Vec<f64> = strokes
        .iter()
        .map(|stroke| (stroke.centre[0] - mean[0]) * ux + (stroke.centre[1] - mean[1]) * uy)
        .collect();
    along_line.sort_by(f64::total_cmp);
    let steps: Vec<f64> = along_line
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .collect();
    let step = steps.iter().sum::<f64>() / steps.len() as f64;
    let uneven = steps.iter().map(|s| (s - step).abs()).fold(0.0, f64::max);
    (lean, off, uneven)
}

/// Strokes may lean this many degrees from square to their line, as rasterizing them does.
const LEAN: f64 = 3.0;
/// Stroke centres may lie this many pixels off their line or off even spacing.
const SPREAD: [f64; 2] = [0.75, 0.5];

/// A terrarium DEM 500 m high everywhere.
fn raised_ground() -> Vec<u8> {
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(256, 256, image::Rgba([129, 244, 0, 255]))
        .write_to(&mut png, image::ImageFormat::Png)
        .expect("PNG");
    png.into_inner()
}

async fn street_map(zoom: f64, terrain: bool) -> SymbolMap {
    let server = AssetServer::default();
    server.serve("https://tiles.test/", street_tile());
    server.serve("https://dem.test/", raised_ground());
    let mut map = SymbolMap::serving(style(zoom, terrain), server).await;
    map.map.set_max_pitch(cgmath::Deg(85.0));
    map
}

/// The strokes of the label at the middle of the view: the stroke nearest it, and every stroke
/// chained to that one through neighbours closer than any other label's.
fn middle_label(strokes: Vec<Stroke>) -> Vec<Stroke> {
    let middle = f64::from(SIZE) / 2.0;
    let distance = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).hypot(a[1] - b[1]);
    let Some(first) = strokes
        .iter()
        .min_by(|a, b| distance(a.centre, [middle; 2]).total_cmp(&distance(b.centre, [middle; 2])))
        .copied()
    else {
        return Vec::new();
    };
    let mut label = vec![first];
    let mut rest: Vec<Stroke> = strokes
        .into_iter()
        .filter(|stroke| stroke.centre != first.centre)
        .collect();
    while let Some(index) = rest.iter().position(|stroke| {
        label
            .iter()
            .any(|member| distance(member.centre, stroke.centre) < 20.0)
    }) {
        label.push(rest.remove(index));
    }
    label
}

/// The name's strokes in `pixels` hold square to one line at even steps, all of them drawn.
fn assert_steady_name(pixels: &[u8], case: &str) {
    let strokes = middle_label(
        strokes(pixels)
            .into_iter()
            .filter(|stroke| stroke.pixels >= 8)
            .collect(),
    );
    assert_eq!(strokes.len(), 16, "{case}: every glyph drawn on its own");
    let (lean, off, uneven) = measure(&strokes);
    assert!(
        lean < LEAN && off < SPREAD[0] && uneven < SPREAD[1],
        "{case}: strokes lean {lean:.2} deg, lie {off:.2} px off their line, step {uneven:.2} px unevenly"
    );
}

#[tokio::test]
async fn a_close_globe_view_keeps_a_street_name_square_to_its_street_and_evenly_spaced() {
    // Zoom 14 lays the street out from its own tile; higher zooms magnify it.
    for zoom in [14.0, 15.0, 17.0, 18.0, 20.0] {
        for pitch in [0.0, 45.0, 70.0, 85.0] {
            let mut map = street_map(zoom, false).await;
            map.map
                .view_state_mut()
                .camera_mut()
                .set_pitch(cgmath::Deg(pitch));
            let pixels = map.settle().await;
            assert_steady_name(&pixels, &format!("zoom {zoom}, pitch {pitch}"));
        }
    }
}

#[tokio::test]
async fn a_free_globe_camera_moving_in_small_steps_keeps_the_street_name_steady() {
    use crate::{projection::ProjectionType, render::view_state::NavigationMode};
    let mut map = street_map(18.0, false).await;
    map.map
        .view_state_mut()
        .camera_mut()
        .set_pitch(cgmath::Deg(45.0));
    map.settle().await;
    map.map
        .view_state_mut()
        .set_navigation_mode(
            NavigationMode::FreeGlobe,
            &ProjectionType::VerticalPerspective,
        )
        .expect("free globe camera");
    let middle = cgmath::Point2::new(f64::from(SIZE) / 2.0, f64::from(SIZE) / 2.0);
    for step in 0..6 {
        let view = map.map.view_state_mut();
        match step % 3 {
            0 => assert!(view.drag_free_globe(middle, middle + cgmath::Vector2::new(3.0, 2.0))),
            1 => assert!(view.zoom_free_globe(middle, 0.15)),
            _ => {
                let pose = view.globe_pose().expect("pose");
                view.orbit_globe_pose(pose, cgmath::Rad(4_f64.to_radians()), cgmath::Rad(0.0))
                    .expect("turn");
            }
        }
        let pixels = map.settle().await;
        assert_steady_name(&pixels, &format!("free camera step {step}"));
    }
}

#[tokio::test]
async fn a_street_name_over_raised_terrain_stays_square_and_evenly_spaced() {
    for zoom in [18.0, 20.0] {
        let mut map = street_map(zoom, true).await;
        map.map
            .view_state_mut()
            .camera_mut()
            .set_pitch(cgmath::Deg(45.0));
        let pixels = map.settle().await;
        assert!(
            (map.map.view_state().center_elevation() - 500.0).abs() < 1.0,
            "the view stands on the 500 m ground"
        );
        assert_steady_name(&pixels, &format!("terrain, zoom {zoom}"));
    }
}
