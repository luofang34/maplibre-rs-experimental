//! Regression tests for orbital globe atmosphere alongside active symbol layers, negative
//! polar symbol `style_zoom` visibility, and high-latitude cyan boundary antimeridian/horizon
//! clipping.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::panic)]

use geozero::mvt::{tile, Message as _, Tile};
use maplibre::{
    background::BackgroundPlugin,
    coords::{LatLon, WorldTileCoords},
    headless::{
        create_headless_renderer,
        map::{process_tile_layers, readback::read_blocking, HeadlessMap, ProcessedLayers},
        HeadlessPlugin,
    },
    plugin::Plugin,
    projection::ProjectionType,
    render::view_state::CameraOptions,
    sdf::{PlacementConfig, SdfPlugin},
    style::Style,
    vector::{DefaultVectorTransferables, VectorPlugin},
    RenderPlugin,
};

const SIZE: u32 = 512;

fn zigzag(value: i64) -> u32 {
    ((value << 1) ^ (value >> 63)) as u32
}

fn synthetic_polar_tile() -> Vec<u8> {
    // High-latitude cyan boundary spanning across tile buffer [-64, 4160] plus a center point
    // label at (2048, 256) (~81N in z=0).
    let mut line_geom = vec![9, zigzag(-64), zigzag(256)];
    let mut prev = (-64_i64, 256_i64);
    line_geom.push((5 << 3) | 2);
    for x in [1024_i64, 2048, 3072, 4096, 4160] {
        line_geom.push(zigzag(x - prev.0));
        line_geom.push(zigzag(256 - prev.1));
        prev = (x, 256);
    }
    let layers = vec![
        tile::Layer {
            version: 2,
            name: "place".to_owned(),
            features: vec![tile::Feature {
                id: Some(1),
                tags: vec![0, 0],
                r#type: Some(tile::GeomType::Point as i32),
                geometry: vec![9, zigzag(2048), zigzag(256)],
            }],
            keys: vec!["name".to_owned()],
            values: vec![tile::Value {
                string_value: Some("ARCTIC".to_owned()),
                ..Default::default()
            }],
            extent: Some(4096),
        },
        tile::Layer {
            version: 2,
            name: "boundary".to_owned(),
            features: vec![tile::Feature {
                id: Some(2),
                tags: Vec::new(),
                r#type: Some(tile::GeomType::Linestring as i32),
                geometry: line_geom,
            }],
            keys: Vec::new(),
            values: Vec::new(),
            extent: Some(4096),
        },
    ];
    Tile { layers }.encode_to_vec()
}

fn polar_style(symbol_minzoom: Option<f64>) -> Style {
    let mut label = serde_json::json!({
        "id": "place-labels",
        "type": "symbol",
        "source": "openmaptiles",
        "source-layer": "place",
        "layout": {
            "text-field": ["get", "name"],
            "text-size": ["interpolate", ["linear"], ["zoom"], 1, 14, 8, 18]
        },
        "paint": {
            "text-color": "#ffffff",
            "text-halo-color": "#09111f",
            "text-halo-width": 1.0
        }
    });
    if let (Some(minzoom), Some(obj)) = (symbol_minzoom, label.as_object_mut()) {
        obj.insert("minzoom".to_owned(), serde_json::json!(minzoom));
    }
    serde_json::from_value(serde_json::json!({
        "version": 8,
        "projection": {"type": "globe"},
        "light": {"anchor": "map", "color": "#ffffff", "intensity": 0.55, "position": [1.5, 210, 45]},
        "sky": {
            "sky-color": "#0b1a36",
            "horizon-color": "#38bdf8",
            "fog-color": "#1e3a5f",
            "atmosphere-blend": ["interpolate", ["linear"], ["zoom"], 0, 0.9, 8, 0]
        },
        "sources": {
            "openmaptiles": {"type": "vector", "tiles": ["http://127.0.0.1:1/{z}/{x}/{y}.mvt"]}
        },
        "layers": [
            {"id": "background", "type": "background", "paint": {"background-color": "#0b172a"}},
            {
                "id": "boundary-country",
                "type": "line",
                "source": "openmaptiles",
                "source-layer": "boundary",
                "paint": {"line-color": "#5eead4", "line-width": 2.0}
            },
            label
        ]
    }))
    .expect("style parses")
}

async fn render_polar_scene(symbol_minzoom: Option<f64>, lat: f64, lon: f64, zoom: f64) -> Vec<u8> {
    let style = polar_style(symbol_minzoom);
    let (kernel, renderer) = create_headless_renderer(SIZE, SIZE, None)
        .await
        .expect("renderer");
    let plugins: Vec<Box<dyn Plugin<_>>> = vec![
        Box::new(RenderPlugin),
        Box::new(BackgroundPlugin),
        Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
        Box::new(SdfPlugin::<DefaultVectorTransferables>::default()),
        Box::new(HeadlessPlugin::new(false).preserve_tile_sources()),
    ];
    let mut map = HeadlessMap::new(style.clone(), renderer, kernel, plugins).expect("map");
    map.set_placement_config(PlacementConfig::immediate());
    map.set_min_zoom(Some(-5.0));

    let tile_bytes = synthetic_polar_tile();
    let mut processed = ProcessedLayers::default();
    for layer in &style.layers {
        if matches!(layer.type_.as_str(), "line" | "symbol") {
            let mut p = process_tile_layers(
                &tile_bytes,
                layer,
                WorldTileCoords::default(),
                ProjectionType::Globe,
            )
            .expect("process");
            processed.append(&mut p);
        }
    }
    map.render_tile(processed).expect("render tile");
    map.jump_to(CameraOptions {
        center: Some(LatLon::new(lat, lon)),
        zoom: Some(zoom),
        bearing: Some(cgmath::Deg(0.0)),
        pitch: Some(cgmath::Deg(18.0)),
        roll: None,
        elevation: None,
    })
    .expect("jump");
    map.run_frame().expect("frame");
    read_blocking(
        &map,
        map.head_texture().expect("head"),
        wgpu::TextureAspect::All,
    )
    .expect("readback")
}

#[tokio::test]
async fn atmosphere_renders_alongside_active_symbol_layer() {
    let pixels = render_polar_scene(None, 38.5, 11.25, 2.1).await;
    // The sunlit northern globe limb must show bright blue atmospheric scattering well above the
    // unlit #0b172a background (blue > 110, exceeding red by > 25), alongside white label glyphs.
    let sunlit_blue_pixels = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|&&[r, _, b, _]| b > 110 && b > r.saturating_add(25))
        .count();
    assert!(
        sunlit_blue_pixels >= 500,
        "expected orbital atmosphere haze alongside symbol layer, found {sunlit_blue_pixels} sunlit pixels"
    );
    let label_pixels = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|&&[r, g, b, _]| r > 210 && g > 210 && b > 220)
        .count();
    assert!(
        label_pixels > 0,
        "expected symbol labels to render alongside atmosphere"
    );
}

#[tokio::test]
async fn negative_polar_zoom_renders_labels_and_clips_boundaries() {
    for minzoom in [None, Some(0.0)] {
        let pixels = render_polar_scene(minzoom, 85.0, 0.0, -1.08).await;
        let label_pixels = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|&&[r, g, b, _]| r > 210 && g > 210 && b > 220)
            .count();
        assert!(
            label_pixels > 0,
            "expected polar labels at z=-1.08 (minzoom={minzoom:?}), found {label_pixels}"
        );
        // Upper corners lie in outer space above the Arctic rim at (512x512, zoom=-1.08, pitch=18)
        // and must have zero stray cyan (#5eead4) chord pixels (empty alpha = 0).
        for (x, y) in [(8_u32, 8_u32), (SIZE - 8, 8)] {
            let off = ((y * SIZE + x) * 4) as usize;
            let px = pixels.get(off..off + 4).unwrap_or(&[]);
            assert_eq!(
                px.get(3).copied(),
                Some(0),
                "expected clear outer-space corner above Arctic rim at ({x},{y}), got {px:?}",
            );
        }
    }
}
