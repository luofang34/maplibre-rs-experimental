#![allow(clippy::expect_used, clippy::panic)]
use crate::{
    coords::{LatLon, WorldTileCoords},
    headless::{create_headless_renderer, map::HeadlessMap},
    render::{
        camera::EyeFrustum,
        view_state::ExternalAnchor,
        xr::{EyeTarget, ScenePlacement, XrEye, XrFrame},
        RenderPlugin,
    },
    style::Style,
};
use cgmath::{Matrix4, Rad, SquareMatrix, Vector3};
use std::time::Duration;

#[tokio::test]
async fn elevated_bridge_draws_over_terrain_and_buried_tunnel_is_occluded() {
    for (kind, elevation, visible) in [("bridge", "1000", true), ("tunnel", "-1000", false)] {
        let mut map = structure_map(kind, elevation).await;
        for timestamp in [0, 16, 32] {
            map.run_xr_frame(XrFrame {
                opaque_environment: true,
                timestamp: Duration::from_millis(timestamp),
                placement: ScenePlacement {
                    anchor: ExternalAnchor {
                        position: LatLon::new(0.0, 0.0),
                        altitude_meters: 0.0,
                    },
                    world_from_scene: Matrix4::identity(),
                },
                eyes: vec![XrEye {
                    world_from_eye: Matrix4::from_translation(Vector3::new(0.0, 0.0, 4000.0)),
                    frustum: EyeFrustum::symmetric(Rad(1.0), 1.0, 0.05, 1e9),
                    target: EyeTarget::default(),
                }],
                request_overscan: 1.0,
                prefetch: None,
            })
            .expect("structure frame");
        }
        let pixels = read_blocking(&map);
        let green = pixels
            .chunks_exact(4)
            .filter(|p| p[1] > 180 && p[0] < 80 && p[2] < 80)
            .count();
        assert_eq!(
            green > 20,
            visible,
            "{kind} elevation {elevation} produced {green} green pixels"
        );
    }
}

async fn structure_map(kind: &str, elevation: &str) -> HeadlessMap {
    let style: Style = serde_json::from_value(serde_json::json!({
            "version": 8, "projection": {"type":"globe"}, "terrain":{"source":"dem"},
            "sources": {
                "roads": {"type":"vector", "tiles":["https://roads.example/{z}/{x}/{y}.pbf"]},
                "dem": {"type":"raster-dem", "tiles":["https://dem.example/{z}/{x}/{y}.png"], "encoding":"terrarium"}
            }, "layers": [
                {"id":"background","type":"background","paint":{"background-color":"#990000"}},
                {"id":"road","type":"line","source":"roads","source-layer":"roads",
                 "metadata":{"maplibre-rs:terrain-structure":kind,"maplibre-rs:structure-elevation-meters":elevation},
                 "paint":{"line-color":"#00ff00","line-width":8}}
            ]
        })).expect("style");
    let layer = style.layers[1].clone();
    assert!(super::kind(&layer).is_some());
    let (kernel, renderer) = create_headless_renderer(64, 64, None)
        .await
        .expect("renderer");
    let mut map = HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(crate::background::BackgroundPlugin),
            Box::new(crate::vector::VectorPlugin::<
                crate::vector::DefaultVectorTransferables,
            >::default()),
            Box::new(crate::terrain::TerrainPlugin::<
                crate::terrain::DefaultDemTransferables,
            >::default()),
            Box::new(crate::headless::HeadlessPlugin::new(false).preserve_tile_sources()),
        ],
    )
    .expect("map");
    let root = WorldTileCoords::default();
    map.load_dem_tiles(vec![(
        root,
        image::RgbaImage::from_pixel(2, 2, image::Rgba([128, 0, 0, 255])),
    )])
    .expect("DEM");
    let line = serde_json::json!({"type":"Feature","properties":{},"geometry":{"type":"LineString","coordinates":[[-0.1,0],[0.1,0]]}});
    let layers = map
        .process_geojson(
            &line,
            "roads",
            vec![layer],
            root,
            crate::projection::ProjectionType::Globe,
        )
        .expect("line");
    assert!(!layers.vector.is_empty(), "processed geometry");
    assert!(
        !layers.vector[0].buffer.buffer.vertices.is_empty(),
        "line vertices"
    );
    map.render_tile(layers).expect("upload line");
    assert!(
        map.world()
            .tiles
            .query::<&crate::vector::VectorLayerBucketComponent>(root)
            .is_some(),
        "root bucket"
    );
    map
}

fn read_blocking(map: &HeadlessMap) -> Vec<u8> {
    let buffer = map.device().create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 64 * 64 * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = map.device().create_command_encoder(&Default::default());
    let texture = map.head_texture().expect("color");
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::ImageCopyBuffer {
            buffer: &buffer,
            layout: wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: None,
            },
        },
        texture.size(),
    );
    map.queue().submit([encoder.finish()]);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, |result| result.expect("readback"));
    map.device().poll(wgpu::Maintain::Wait);
    let bytes = buffer.slice(..).get_mapped_range().to_vec();
    buffer.unmap();
    bytes
}
