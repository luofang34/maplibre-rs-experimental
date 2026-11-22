//! Integration tests for C6 symbol placement config & GPU fade transitions (`gtest_symbol_rendering`):
//! 1. Immediate placement renders full ink on frame 1.
//! 2. Timed fade-in increases ink monotonically across 0ms -> 150ms -> 350ms.
//! 3. Collision fade-out decreases ink monotonically to 0 when obstructed.
//! 4. `text-allow-overlap: true` renders colliding labels simultaneously.
//! 5. Long-session timestamp (> 35 min) rebases fade clock without losing or freezing fades.
//! 6. Fade duration > 1s retains fading state past 1000ms to completion.
//! 7. Variable anchor (`text-variable-anchor`) relocates a colliding label to an open anchor.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::panic)]

use std::time::Duration;

use maplibre::{
    background::BackgroundPlugin,
    coords::WorldTileCoords,
    headless::{
        create_headless_renderer_with_settings,
        map::{
            process_geojson_layers,
            readback::{FrameReadback, PixelLayout},
            HeadlessMap,
        },
        HeadlessPlugin,
    },
    plugin::Plugin,
    projection::ProjectionType,
    render::{
        settings::{Msaa, RendererSettings},
        RenderPlugin,
    },
    sdf::{PlacementConfig, SdfPlugin},
    style::Style,
    vector::{DefaultVectorTransferables, VectorPlugin},
};

const SIZE: u32 = 512;

fn symbol_style(extra_layout: serde_json::Map<String, serde_json::Value>) -> Style {
    let mut layout = serde_json::Map::new();
    layout.insert("text-field".into(), serde_json::json!("{label}"));
    layout.insert("text-size".into(), serde_json::json!(28));
    layout.extend(extra_layout);
    serde_json::from_value(serde_json::json!({
        "version": 8,
        "center": [0.0, 0.0],
        "zoom": 0,
        "sources": {
            "pts": {
                "type": "geojson",
                "data": { "type": "FeatureCollection", "features": [] }
            }
        },
        "layers": [
            {
                "id": "paper",
                "type": "background",
                "paint": { "background-color": "#ffffff" }
            },
            {
                "id": "label",
                "type": "symbol",
                "source": "pts",
                "layout": layout,
                "paint": { "text-color": "#000000" }
            }
        ]
    }))
    .expect("symbol style")
}

fn single_point_geojson(label: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "FeatureCollection",
        "features": [{
            "type": "Feature",
            "geometry": { "type": "Point", "coordinates": [0.0, 0.0] },
            "properties": { "label": label }
        }]
    })
}

fn two_point_geojson(a: &str, b: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "FeatureCollection",
        "features": [
            {
                "type": "Feature",
                "geometry": { "type": "Point", "coordinates": [0.0, 0.0] },
                "properties": { "label": a }
            },
            {
                "type": "Feature",
                "geometry": { "type": "Point", "coordinates": [0.0, 0.0] },
                "properties": { "label": b }
            }
        ]
    })
}

async fn setup_map(style: Style) -> (HeadlessMap, FrameReadback) {
    let settings = RendererSettings {
        msaa: Msaa { samples: 1 },
        ..Default::default()
    };
    let (kernel, renderer) = create_headless_renderer_with_settings(SIZE, SIZE, None, settings)
        .await
        .expect("headless renderer");
    let plugins: Vec<Box<dyn Plugin<_>>> = vec![
        Box::new(RenderPlugin),
        Box::new(BackgroundPlugin),
        Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
        Box::new(SdfPlugin::<DefaultVectorTransferables>::default()),
        Box::new(HeadlessPlugin::new(false)),
    ];
    let map = HeadlessMap::new(style, renderer, kernel, plugins).expect("headless map");
    let readback = FrameReadback::new(&map, SIZE, SIZE);
    (map, readback)
}

fn render_geojson(map: &mut HeadlessMap, geojson: &serde_json::Value) {
    let label = map
        .context()
        .style
        .layers
        .iter()
        .find(|layer| layer.id == "label")
        .cloned()
        .expect("label layer");
    let layers = process_geojson_layers(
        geojson,
        "pts",
        vec![label],
        WorldTileCoords::default(),
        ProjectionType::Mercator,
    )
    .expect("process geojson");
    assert_eq!(layers.symbols.len(), 1, "expected symbol bucket");
    map.render_tile(layers).expect("render tile");
}

fn ink_in_rect(
    map: &HeadlessMap,
    readback: &mut FrameReadback,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
) -> u64 {
    let mut pixels = vec![0_u8; readback.frame_len(PixelLayout::Rgba8)];
    readback
        .read_into_blocking(
            map,
            map.head_texture().expect("head texture"),
            PixelLayout::Rgba8,
            &mut pixels,
        )
        .expect("readback");
    let mut sum = 0_u64;
    for y in y0..y1 {
        for x in x0..x1 {
            let idx = ((y * SIZE + x) * 4) as usize;
            let r = pixels.get(idx).copied().expect("r byte");
            let g = pixels.get(idx + 1).copied().expect("g byte");
            let b = pixels.get(idx + 2).copied().expect("b byte");
            let min_rgb = r.min(g).min(b);
            sum += u64::from(255 - min_rgb);
        }
    }
    sum
}

fn center_ink(map: &HeadlessMap, readback: &mut FrameReadback) -> u64 {
    ink_in_rect(map, readback, 160, 160, 352, 352)
}

#[tokio::test]
async fn immediate_placement_renders_full_ink_on_frame_one() {
    let style = symbol_style(serde_json::Map::new());
    let (mut map, mut readback) = setup_map(style).await;
    map.set_placement_config(PlacementConfig::immediate());

    render_geojson(&mut map, &single_point_geojson("V12"));
    let frame1 = center_ink(&map, &mut readback);
    assert!(
        frame1 > 5_000,
        "immediate mode should render full ink on frame 1, got {frame1}"
    );

    map.frame_input_mut().advance(Duration::from_millis(200));
    map.run_frame().expect("settled frame");
    let frame2 = center_ink(&map, &mut readback);
    assert_eq!(frame1, frame2, "frame 1 should already be at 100% opacity");
}

#[tokio::test]
async fn timed_fade_in_increases_ink_monotonically() {
    let style = symbol_style(serde_json::Map::new());
    let (mut map, mut readback) = setup_map(style).await;
    map.set_placement_config(PlacementConfig {
        fade_duration: Duration::from_millis(300),
    });

    render_geojson(&mut map, &single_point_geojson("V12"));
    let t0 = center_ink(&map, &mut readback);
    assert_eq!(t0, 0, "newly placed symbol starts fade-in at 0 opacity");

    map.frame_input_mut().advance(Duration::from_millis(150));
    map.run_frame().expect("mid fade");
    let t150 = center_ink(&map, &mut readback);

    map.frame_input_mut().advance(Duration::from_millis(200));
    map.run_frame().expect("settled fade");
    let t350 = center_ink(&map, &mut readback);

    assert!(
        t150 > t0 && t350 > t150,
        "expected monotonic fade-in: 0={t0}, 150={t150}, 350={t350}"
    );
}

#[tokio::test]
async fn collision_fade_out_decreases_ink_monotonically_to_zero() {
    let style = symbol_style(serde_json::Map::new());
    let (mut map, mut readback) = setup_map(style).await;
    map.set_placement_config(PlacementConfig {
        fade_duration: Duration::from_millis(300),
    });

    render_geojson(&mut map, &single_point_geojson("V12"));
    map.frame_input_mut().advance(Duration::from_millis(350));
    map.run_frame().expect("settled fade-in");
    let full = center_ink(&map, &mut readback);
    assert!(
        full > 5_000,
        "expected full ink before obstacle, got {full}"
    );

    map.set_label_obstacles(vec![[180.0, 180.0, 332.0, 332.0]]);
    map.frame_input_mut().advance(Duration::from_millis(16));
    map.run_frame().expect("fade-out starts");

    map.frame_input_mut().advance(Duration::from_millis(150));
    map.run_frame().expect("fade-out mid");
    let mid = center_ink(&map, &mut readback);

    map.frame_input_mut().advance(Duration::from_millis(200));
    map.run_frame().expect("fade-out end");
    let final_ink = center_ink(&map, &mut readback);

    assert!(
        full > mid && mid > final_ink && final_ink == 0,
        "expected fade-out: full={full}, mid={mid}, final={final_ink}"
    );
}

#[tokio::test]
async fn allow_overlap_renders_both_colliding_symbols() {
    let geojson = two_point_geojson("III", "WWW");

    let (mut map_no_overlap, mut rb_no) = setup_map(symbol_style(serde_json::Map::new())).await;
    map_no_overlap.set_placement_config(PlacementConfig::immediate());
    render_geojson(&mut map_no_overlap, &geojson);
    assert_eq!(
        map_no_overlap
            .query_rendered_symbols([256.0, 256.0], None)
            .len(),
        1,
        "without allow-overlap only one colliding symbol places"
    );
    let ink_single = center_ink(&map_no_overlap, &mut rb_no);

    let mut overlap_layout = serde_json::Map::new();
    overlap_layout.insert("text-allow-overlap".into(), serde_json::json!(true));
    let (mut map_overlap, mut rb_yes) = setup_map(symbol_style(overlap_layout)).await;
    map_overlap.set_placement_config(PlacementConfig::immediate());
    render_geojson(&mut map_overlap, &geojson);
    assert_eq!(
        map_overlap
            .query_rendered_symbols([256.0, 256.0], None)
            .len(),
        2,
        "with allow-overlap both colliding symbols place"
    );
    let ink_both = center_ink(&map_overlap, &mut rb_yes);
    assert!(
        ink_both > ink_single,
        "both overlapped symbols should draw more ink ({ink_both} vs {ink_single})"
    );
}

#[tokio::test]
async fn long_session_past_35_minutes_rebases_clock_and_keeps_fading() {
    let style = symbol_style(serde_json::Map::new());
    let (mut map, mut readback) = setup_map(style).await;
    map.set_placement_config(PlacementConfig {
        fade_duration: Duration::from_millis(300),
    });

    render_geojson(&mut map, &single_point_geojson("V12"));
    // Advance > 36.6 min while fade-in is active so the fade clock jumps past 2_097_148 ms and
    // triggers rebase back to 0 without dropping settled glyphs or freezing future fades.
    map.frame_input_mut()
        .advance(Duration::from_millis(2_200_000));
    map.run_frame().expect("post-rebase frame");
    let settled = center_ink(&map, &mut readback);
    assert!(
        settled > 5_000,
        "settled symbol must stay visible after rebase, got {settled}"
    );

    map.set_label_obstacles(vec![[180.0, 180.0, 332.0, 332.0]]);
    map.frame_input_mut().advance(Duration::from_millis(16));
    map.run_frame().expect("fade-out start");
    map.frame_input_mut().advance(Duration::from_millis(150));
    map.run_frame().expect("fade-out mid");
    let mid = center_ink(&map, &mut readback);
    map.frame_input_mut().advance(Duration::from_millis(200));
    map.run_frame().expect("fade-out complete");
    let done = center_ink(&map, &mut readback);

    assert!(
        settled > mid && mid > done && done == 0,
        "post-rebase fade-out stalled: settled={settled}, mid={mid}, done={done}"
    );
}

#[tokio::test]
async fn fade_duration_over_one_second_continues_past_1000ms() {
    let style = symbol_style(serde_json::Map::new());
    let (mut map, mut readback) = setup_map(style).await;
    map.set_placement_config(PlacementConfig {
        fade_duration: Duration::from_millis(1500),
    });

    render_geojson(&mut map, &single_point_geojson("V12"));
    let t0 = center_ink(&map, &mut readback);
    map.frame_input_mut().advance(Duration::from_millis(750));
    map.run_frame().expect("750ms");
    let t750 = center_ink(&map, &mut readback);
    map.frame_input_mut().advance(Duration::from_millis(500));
    map.run_frame().expect("1250ms");
    let t1250 = center_ink(&map, &mut readback);
    map.frame_input_mut().advance(Duration::from_millis(450));
    map.run_frame().expect("1700ms");
    let t1700 = center_ink(&map, &mut readback);

    assert!(
        t0 == 0 && t750 > t0 && t1250 > t750 && t1700 > t1250,
        "expected monotonic fade across 1s boundary: t0={t0}, t750={t750}, t1250={t1250}, t1700={t1700}"
    );
}

#[tokio::test]
async fn variable_anchor_moves_obstructed_label_to_alternate_anchor() {
    let mut layout = serde_json::Map::new();
    layout.insert(
        "text-variable-anchor".into(),
        serde_json::json!(["top", "bottom"]),
    );
    layout.insert("text-radial-offset".into(), serde_json::json!(1.2));
    layout.insert("text-justify".into(), serde_json::json!("auto"));
    let style = symbol_style(layout);
    let (mut map, mut readback) = setup_map(style).await;
    map.set_placement_config(PlacementConfig::immediate());

    render_geojson(&mut map, &single_point_geojson("V12"));
    let lower_before = ink_in_rect(&map, &mut readback, 160, 256, 352, 400);
    let upper_before = ink_in_rect(&map, &mut readback, 160, 112, 352, 256);
    assert!(
        lower_before > 3_000 && upper_before == 0,
        "primary 'top' anchor places below center: lower={lower_before}, upper={upper_before}"
    );

    // Block the lower half where 'top' placed the label, leaving upper half free for 'bottom'.
    map.set_label_obstacles(vec![[160.0, 258.0, 352.0, 400.0]]);
    map.run_frame().expect("relocate to alternate anchor");

    let lower_after = ink_in_rect(&map, &mut readback, 160, 256, 352, 400);
    let upper_after = ink_in_rect(&map, &mut readback, 160, 112, 352, 256);
    assert!(
        lower_after == 0 && upper_after > 3_000,
        "obstructed label must flip to 'bottom' anchor above center: lower={lower_after}, upper={upper_after}"
    );
}
