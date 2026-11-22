//! Renders one labelled point through the headless map and checks that its text reaches the
//! frame, so symbol buckets are not only returned by processing but also drawn.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::panic)]

use maplibre::{
    background::BackgroundPlugin,
    coords::WorldTileCoords,
    headless::{
        create_headless_renderer,
        map::{process_tile_layers, readback::read_blocking, HeadlessMap},
        HeadlessPlugin,
    },
    plugin::Plugin,
    projection::ProjectionType,
    render::RenderPlugin,
    sdf::{PlacementConfig, SdfPlugin},
    style::Style,
    vector::{DefaultVectorTransferables, VectorPlugin},
};

/// A `route_points` layer with one point at the tile centre, named `FALLBACK` and labelled
/// `V12`.
const LABELLED_POINT_TILE: &str = "1a460a0c726f7574655f706f696e7473121108011204000001011801220509802080201a046e616d651a056c6162656c220a0a0846414c4c4241434b22050a035631322880207802";
const SIZE: u32 = 512;

fn hex_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).expect("hex digit pair"))
        .collect()
}

/// Pixels of the central region that are not the white background.
fn inked_centre_pixels(width: u32, pixels: &[u8]) -> usize {
    let centre = SIZE / 2;
    let half = 48;
    let mut inked = 0;
    for y in centre - half..centre + half {
        for x in centre - half..centre + half {
            let offset = ((y * width + x) * 4) as usize;
            let [r, g, b] = [
                pixels.get(offset).copied().unwrap_or(255),
                pixels.get(offset + 1).copied().unwrap_or(255),
                pixels.get(offset + 2).copied().unwrap_or(255),
            ];
            if r < 200 || g < 200 || b < 200 {
                inked += 1;
            }
        }
    }
    inked
}

#[tokio::test]
async fn a_labelled_point_renders_its_text_headless() {
    let style: Style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "center": [0.0, 0.0],
        "zoom": 0,
        "sources": {
            "chart": {"type": "vector", "tiles": ["http://127.0.0.1:1/{z}/{x}/{y}.mvt"]}
        },
        "layers": [
            {"id": "paper", "type": "background", "paint": {"background-color": "#ffffff"}},
            {
                "id": "label", "type": "symbol", "source": "chart",
                "source-layer": "route_points", "layout": {"text-field": "{label}"}
            }
        ]
    }))
    .expect("style parses");
    let label = style
        .layers
        .iter()
        .find(|layer| layer.id == "label")
        .cloned()
        .expect("label layer");

    let (kernel, renderer) = create_headless_renderer(SIZE, SIZE, None)
        .await
        .expect("headless renderer");
    let plugins: Vec<Box<dyn Plugin<_>>> = vec![
        Box::new(RenderPlugin),
        Box::new(BackgroundPlugin),
        Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
        Box::new(SdfPlugin::<DefaultVectorTransferables>::default()),
        Box::new(HeadlessPlugin::new(false)),
    ];
    let mut map = HeadlessMap::new(style, renderer, kernel, plugins).expect("headless map");
    map.set_placement_config(PlacementConfig::immediate());

    let layers = process_tile_layers(
        &hex_bytes(LABELLED_POINT_TILE),
        &label,
        WorldTileCoords::default(),
        ProjectionType::Mercator,
    )
    .expect("tile processes");
    assert_eq!(
        layers.symbols.len(),
        1,
        "the label's symbol bucket survives processing"
    );

    map.render_tile(layers).expect("frame renders");
    let pixels = read_blocking(
        &map,
        map.head_texture().expect("head texture"),
        wgpu::TextureAspect::All,
    )
    .expect("readback");

    let inked = inked_centre_pixels(SIZE, &pixels);
    assert!(
        inked > 0,
        "the label's glyphs leave ink at the tile centre, found {inked} inked pixels"
    );
}

#[tokio::test]
async fn a_label_under_a_host_obstacle_is_not_placed() {
    let style: Style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "center": [0.0, 0.0],
        "zoom": 0,
        "sources": {
            "chart": {"type": "vector", "tiles": ["http://127.0.0.1:1/{z}/{x}/{y}.mvt"]}
        },
        "layers": [
            {"id": "paper", "type": "background", "paint": {"background-color": "#ffffff"}},
            {
                "id": "label", "type": "symbol", "source": "chart",
                "source-layer": "route_points", "layout": {"text-field": "{label}"}
            }
        ]
    }))
    .expect("style parses");
    let label = style
        .layers
        .iter()
        .find(|layer| layer.id == "label")
        .cloned()
        .expect("label layer");
    let (kernel, renderer) = create_headless_renderer(SIZE, SIZE, None)
        .await
        .expect("headless renderer");
    let plugins: Vec<Box<dyn Plugin<_>>> = vec![
        Box::new(RenderPlugin),
        Box::new(BackgroundPlugin),
        Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
        Box::new(SdfPlugin::<DefaultVectorTransferables>::default()),
        Box::new(HeadlessPlugin::new(false)),
    ];
    let mut map = HeadlessMap::new(style, renderer, kernel, plugins).expect("headless map");
    map.set_placement_config(PlacementConfig::immediate());
    let tile = || {
        process_tile_layers(
            &hex_bytes(LABELLED_POINT_TILE),
            &label,
            WorldTileCoords::default(),
            ProjectionType::Mercator,
        )
        .expect("tile processes")
    };
    let centre = [f64::from(SIZE) / 2.0; 2];

    map.render_tile(tile()).expect("frame renders");
    assert_eq!(
        map.query_rendered_symbols(centre, None).len(),
        1,
        "placed in the open"
    );

    let half = f64::from(SIZE) / 8.0;
    map.set_label_obstacles(vec![[
        centre[0] - half,
        centre[1] - half,
        centre[0] + half,
        centre[1] + half,
    ]]);
    map.render_tile(tile()).expect("frame renders");
    assert!(
        map.query_rendered_symbols(centre, None).is_empty(),
        "not placed under the host's obstacle"
    );
}

#[tokio::test]
async fn a_query_measures_in_logical_pixels_at_a_pixel_ratio_of_two() {
    let style: Style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "center": [0.0, 0.0],
        "zoom": 0,
        "sources": {
            "chart": {"type": "vector", "tiles": ["http://127.0.0.1:1/{z}/{x}/{y}.mvt"]}
        },
        "layers": [
            {
                "id": "label", "type": "symbol", "source": "chart",
                "source-layer": "route_points", "layout": {"text-field": "{label}"}
            }
        ]
    }))
    .expect("style parses");
    let label = style.layers.first().expect("label layer").clone();
    let (kernel, renderer) = create_headless_renderer(SIZE, SIZE, None)
        .await
        .expect("headless renderer");
    let plugins: Vec<Box<dyn Plugin<_>>> = vec![
        Box::new(RenderPlugin),
        Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
        Box::new(SdfPlugin::<DefaultVectorTransferables>::default()),
        Box::new(HeadlessPlugin::new(false)),
    ];
    let mut map = HeadlessMap::new(style, renderer, kernel, plugins).expect("headless map");
    map.set_placement_config(PlacementConfig::immediate());
    map.set_pixel_ratio(2.0);
    let layers = process_tile_layers(
        &hex_bytes(LABELLED_POINT_TILE),
        &label,
        WorldTileCoords::default(),
        ProjectionType::Mercator,
    )
    .expect("tile processes");
    map.render_tile(layers).expect("frame renders");

    // The page's CSS pixels: a browser host passes pointer positions through unscaled.
    let logical_centre = [f64::from(SIZE) / 4.0; 2];
    assert_eq!(
        map.query_rendered_symbols(logical_centre, None).len(),
        1,
        "the label sits at the centre of the logical viewport"
    );
    let physical_centre = [f64::from(SIZE) / 2.0; 2];
    assert!(
        map.query_rendered_symbols(physical_centre, None).is_empty(),
        "the physical centre is the logical viewport's far corner"
    );
}
