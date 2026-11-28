//! Headless render regression tests verifying `line-dasharray` fractional-zoom ground anchoring
//! and integer-zoom cross-fade stability.

#![cfg(not(target_arch = "wasm32"))]

use std::{error::Error, time::Duration};

use maplibre::{
    background::BackgroundPlugin,
    coords::LatLon,
    headless::{
        create_headless_renderer_with_settings,
        map::{readback::read_blocking, HeadlessMap, ProcessedLayers},
        HeadlessPlugin,
    },
    plugin::Plugin,
    projection::ProjectionType,
    render::{
        settings::{Msaa, RendererSettings},
        view_state::CameraOptions,
    },
    style::Style,
    vector::{DefaultVectorTransferables, VectorPlugin},
    RenderPlugin,
};

const WIDTH: u32 = 512;
const HEIGHT: u32 = 128;
const LAT: f64 = -0.0014;
const STEP: Duration = Duration::from_millis(16);

fn dashed_style(initial_zoom: f64) -> Result<Style, Box<dyn Error>> {
    let style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "center": [0.0, LAT],
        "zoom": initial_zoom,
        "sources": {
            "geojson": {
                "type": "geojson",
                "data": {
                    "type": "Feature",
                    "properties": {},
                    "geometry": {
                        "type": "LineString",
                        "coordinates": [[-0.02, LAT], [0.02, LAT]]
                    }
                }
            }
        },
        "layers": [
            {
                "id": "background",
                "type": "background",
                "paint": { "background-color": "#000000" }
            },
            {
                "id": "dashed",
                "type": "line",
                "source": "geojson",
                "paint": {
                    "line-color": "#ffffff",
                    "line-width": ["interpolate", ["exponential", 1.2], ["zoom"], 14, 0.5, 20, 10],
                    "line-dasharray": [1, 0.75]
                }
            }
        ]
    }))?;
    Ok(style)
}

fn line_geojson() -> serde_json::Value {
    serde_json::json!({
        "type": "Feature",
        "properties": {},
        "geometry": {
            "type": "LineString",
            "coordinates": [[-0.02, LAT], [0.02, LAT]]
        }
    })
}

async fn create_test_map(initial_zoom: f64) -> Result<(HeadlessMap, Style), Box<dyn Error>> {
    let style = dashed_style(initial_zoom)?;
    let settings = RendererSettings {
        msaa: Msaa { samples: 1 },
        ..RendererSettings::default()
    };
    let (kernel, renderer) =
        create_headless_renderer_with_settings(WIDTH, HEIGHT, None, settings).await?;
    let plugins: Vec<Box<dyn Plugin<_>>> = vec![
        Box::new(RenderPlugin),
        Box::new(BackgroundPlugin),
        Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
        Box::new(HeadlessPlugin::new(false).preserve_tile_sources()),
    ];
    let map = HeadlessMap::new(style.clone(), renderer, kernel, plugins)?;
    Ok((map, style))
}

fn load_required_tiles(map: &mut HeadlessMap, style: &Style) -> Result<(), Box<dyn Error>> {
    let mut processed = ProcessedLayers::default();
    let geojson = line_geojson();
    for coords in map.required_tile_coords()? {
        let mut tile = map.process_geojson(
            &geojson,
            "geojson",
            style.layers.clone(),
            coords,
            ProjectionType::Mercator,
        )?;
        processed.append(&mut tile);
    }
    map.render_tile(processed)?;
    Ok(())
}

fn step_frame(map: &mut HeadlessMap, by: Duration) -> Result<(), Box<dyn Error>> {
    map.frame_input_mut().advance(by);
    map.run_frame()?;
    Ok(())
}

fn read_center_row_red(map: &HeadlessMap) -> Result<Vec<u8>, Box<dyn Error>> {
    let tex = map
        .head_texture()
        .ok_or_else(|| std::io::Error::other("missing head texture"))?;
    let pixels = read_blocking(map, tex, wgpu::TextureAspect::All)?;
    let y = (HEIGHT / 2) as usize;
    let start = y * WIDTH as usize * 4;
    let mut row = Vec::with_capacity(WIDTH as usize);
    for x in 0..WIDTH as usize {
        let idx = start + x * 4;
        row.push(pixels.get(idx).copied().unwrap_or(0));
    }
    Ok(row)
}

fn run_midpoints(row: &[u8], x0: usize, x1: usize, pred: impl Fn(u8) -> bool) -> Vec<f64> {
    let mut midpoints = Vec::new();
    let mut run_start: Option<usize> = None;
    for (x, &val) in row.iter().enumerate().take(x1).skip(x0) {
        if pred(val) {
            if run_start.is_none() {
                run_start = Some(x);
            }
        } else if let Some(start) = run_start.take() {
            midpoints.push((start + x - 1) as f64 * 0.5);
        }
    }
    if let Some(start) = run_start {
        midpoints.push((start + x1 - 1) as f64 * 0.5);
    }
    midpoints
}

#[tokio::test]
async fn dashed_line_stays_anchored_to_ground_during_fractional_zoom() -> Result<(), Box<dyn Error>>
{
    let (mut map, style) = create_test_map(16.1).await?;
    load_required_tiles(&mut map, &style)?;
    step_frame(&mut map, STEP)?;
    let row_161 = read_center_row_red(&map)?;

    let cx = f64::from(WIDTH) * 0.5;
    let dashes = run_midpoints(&row_161, 136, 376, |r| r >= 160);
    let gaps = run_midpoints(&row_161, 136, 376, |r| r <= 80);
    assert!(
        dashes.len() >= 8 && gaps.len() >= 8,
        "expected >= 8 dashes and gaps at z=16.1, got {} dashes and {} gaps",
        dashes.len(),
        gaps.len()
    );

    map.jump_to(CameraOptions {
        center: Some(LatLon::new(LAT, 0.0)),
        zoom: Some(16.8),
        bearing: None,
        pitch: None,
        roll: None,
        elevation: None,
    })?;
    step_frame(&mut map, STEP)?;
    let row_168 = read_center_row_red(&map)?;

    let scale = 2.0_f64.powf(16.8 - 16.1);
    for (label, list, expected_bright) in [("dash", dashes, true), ("gap", gaps, false)] {
        for x_161 in list {
            let x_168 = (cx + (x_161 - cx) * scale).round();
            if x_168 < 16.0 || x_168 >= f64::from(WIDTH - 16) {
                continue;
            }
            let xi = x_168 as usize;
            let r = row_168.get(xi).copied().unwrap_or(128);
            if expected_bright {
                assert!(
                    r >= 160,
                    "projected {label} midpoint at x={xi} (from {x_161:.1}) swam/darkened: R={r}"
                );
            } else {
                assert!(
                    r <= 80,
                    "projected {label} midpoint at x={xi} (from {x_161:.1}) swam/lit up: R={r}"
                );
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn integer_zoom_crossing_crossfades_cleanly_and_settles() -> Result<(), Box<dyn Error>> {
    let (mut map, style) = create_test_map(15.95).await?;
    load_required_tiles(&mut map, &style)?;
    step_frame(&mut map, STEP)?;
    let row_before = read_center_row_red(&map)?;
    let bright_before = row_before.iter().filter(|&&r| r >= 120).count();
    let dark_before = row_before.iter().filter(|&&r| r <= 80).count();
    assert!(bright_before > 30 && dark_before > 30);

    // Preload z=16 tiles before crossing so the mid-fade frame already has full geometry.
    map.jump_to(CameraOptions {
        center: Some(LatLon::new(LAT, 0.0)),
        zoom: Some(16.05),
        bearing: None,
        pitch: None,
        roll: None,
        elevation: None,
    })?;
    load_required_tiles(&mut map, &style)?;
    let mid_up = read_center_row_red(&map)?;
    let bright_mid_up = mid_up.iter().filter(|&&r| r >= 120).count();
    let dark_mid_up = mid_up.iter().filter(|&&r| r <= 80).count();
    assert!(
        bright_mid_up > 20 && dark_mid_up > 20,
        "mid-fade up must not go blank or solid: bright={bright_mid_up}, dark={dark_mid_up}"
    );

    for _ in 0..20 {
        step_frame(&mut map, STEP)?;
    }
    let settled_up = read_center_row_red(&map)?;
    let bright_up = settled_up.iter().filter(|&&r| r >= 160).count();
    let dark_up = settled_up.iter().filter(|&&r| r <= 80).count();
    assert!(bright_up > 30 && dark_up > 30);
    assert!(
        !map.needs_redraw(),
        "settled integer-zoom crossfade must stop requesting frames"
    );

    // Cross back down across integer zoom 16 -> 15.95.
    map.jump_to(CameraOptions {
        center: Some(LatLon::new(LAT, 0.0)),
        zoom: Some(15.95),
        bearing: None,
        pitch: None,
        roll: None,
        elevation: None,
    })?;
    step_frame(&mut map, STEP)?;
    let mid_down = read_center_row_red(&map)?;
    let bright_mid_down = mid_down.iter().filter(|&&r| r >= 120).count();
    let dark_mid_down = mid_down.iter().filter(|&&r| r <= 80).count();
    assert!(
        bright_mid_down > 20 && dark_mid_down > 20,
        "mid-fade down must not go blank or solid: bright={bright_mid_down}, dark={dark_mid_down}"
    );

    for _ in 0..20 {
        step_frame(&mut map, STEP)?;
    }
    assert!(
        !map.needs_redraw(),
        "settled downward crossfade must stop requesting frames"
    );
    Ok(())
}
