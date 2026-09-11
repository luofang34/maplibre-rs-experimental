#![allow(clippy::expect_used, clippy::panic)]
use super::{read_back_blocking, SIZE};
use crate::{
    coords::WorldTileCoords,
    headless::{
        create_headless_renderer,
        map::{process_tile_layers, HeadlessMap},
    },
    render::RenderPlugin,
    style::Style,
};
use geozero::mvt::Message;

#[tokio::test]
async fn globe_background_does_not_occlude_below_sea_level_terrain() {
    for height in [-300.0, 300.0] {
        let map = water_map(height, 0).await;
        let pixels = read_back_blocking(&map);
        let offset = ((SIZE / 2 * SIZE + SIZE / 2) * 4) as usize;
        let center = &pixels[offset..offset + 4];
        assert!(
            center[2] > 150 && center[0] < 80,
            "water at {height}m must cover the red background: {center:?}"
        );
    }
}

async fn water_map(height: f64, source_zoom: u8) -> HeadlessMap {
    let source_coords = WorldTileCoords {
        x: (1_i32 << source_zoom) / 2,
        y: (1_i32 << source_zoom) / 2,
        z: source_zoom.into(),
    };
    let style: Style = serde_json::from_value(serde_json::json!({
        "version": 8, "zoom": if source_zoom == 0 { 3 } else { 12 }, "center": [0.04,-0.04], "projection": {"type":"globe"},
        "sources": {
            "water": {"type":"vector", "tiles":["https://unused.invalid/{z}/{x}/{y}"], "maxzoom":source_zoom},
            "dem": {"type":"raster-dem", "tiles":["https://unused.invalid/dem/{z}/{x}/{y}"], "encoding":"terrarium", "maxzoom":0}
        },
        "terrain":{"source":"dem"}, "layers":[
            {"id":"background", "type":"background", "paint":{"background-color":"#ff0000"}},
            {"id":"water", "type":"fill", "source":"water", "source-layer":"water", "paint":{"fill-color":"#0000ff"}}
        ]
    })).expect("style");
    let source = geozero::mvt::tile::Layer {
        name: "water".into(),
        version: 2,
        extent: Some(4096),
        features: vec![geozero::mvt::tile::Feature {
            r#type: Some(3),
            geometry: vec![9, 0, 0, 26, 8192, 0, 0, 8192, 8191, 0, 15],
            ..Default::default()
        }],
        ..Default::default()
    };
    let bytes = geozero::mvt::Tile {
        layers: vec![source],
    }
    .encode_to_vec();
    let layers = process_tile_layers(
        &bytes,
        &style.layers[1],
        source_coords,
        crate::projection::ProjectionType::Globe,
    )
    .expect("water mesh");
    let (kernel, renderer) = create_headless_renderer(SIZE, SIZE, None)
        .await
        .expect("renderer");
    let mut map = HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(crate::vector::VectorPlugin::<
                crate::vector::DefaultVectorTransferables,
            >::default()),
            Box::new(crate::background::BackgroundPlugin),
            Box::new(crate::terrain::TerrainPlugin::<
                crate::terrain::DefaultDemTransferables,
            >::default()),
            Box::new(crate::headless::HeadlessPlugin::new(false).preserve_tile_sources()),
        ],
    )
    .expect("map");
    let encoded = height + 32768.0;
    let pixel = image::Rgba([(encoded / 256.0) as u8, (encoded % 256.0) as u8, 0, 255]);
    map.render_frames_with_terrain(
        layers,
        vec![],
        vec![(
            WorldTileCoords::default(),
            image::RgbaImage::from_pixel(16, 16, pixel),
        )],
        16,
    )
    .expect("terrain frame");
    map
}

#[tokio::test]
async fn overzoomed_terrain_uploads_the_available_vector_ancestor() {
    let map = water_map(300.0, 6).await;
    let pixels = read_back_blocking(&map);
    let offset = ((SIZE / 2 * SIZE + SIZE / 2) * 4) as usize;
    let center = &pixels[offset..offset + 4];
    assert!(
        center[2] > 150 && center[0] < 80,
        "zoom 12 must paint the available z6 source over the red fallback: {center:?}"
    );
}

#[tokio::test]
async fn memory_pressure_preserves_the_overzoomed_ground_paint() {
    let mut map = water_map(300.0, 6).await;
    for available in [768_u64 << 20, 300_u64 << 20] {
        map.set_available_memory(Some(available));
        for _ in 0..16 {
            map.run_frame().expect("constrained frame");
            let pixels = read_back_blocking(&map);
            let offset = ((SIZE / 2 * SIZE + SIZE / 2) * 4) as usize;
            assert!(
                pixels[offset + 2] > 150 && pixels[offset] < 80,
                "memory pressure must retain painted ground while drape limits change"
            );
        }
    }
}
