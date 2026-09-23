#![allow(clippy::expect_used, clippy::panic)]

use cgmath::{Matrix4, Rad, SquareMatrix, Vector3};
use image::{Rgba, RgbaImage};

use crate::{
    coords::{LatLon, WorldTileCoords},
    headless::{
        create_headless_renderer,
        map::{HeadlessMap, ProcessedLayers},
        HeadlessPlugin,
    },
    raster::{AvailableRasterLayerData, DefaultRasterTransferables, RasterPlugin},
    render::{
        camera::EyeFrustum,
        view_state::ExternalAnchor,
        xr::{EyeTarget, ScenePlacement, XrEye, XrFrame},
        RenderPlugin,
    },
    style::Style,
    terrain::{DefaultDemTransferables, TerrainPlugin},
};

const SIZE: u32 = 512;
const TILE: (i32, i32, u8) = (38428, 49355, 17);

#[tokio::test]
async fn low_altitude_globe_preserves_detailed_raster_without_mesh_distortion() {
    let flat = render("mercator").await;
    let globe = render("vertical-perspective").await;
    let mut differences = Vec::new();
    for y in 64..SIZE - 64 {
        for x in 64..SIZE - 64 {
            let i = ((y * SIZE + x) * 4) as usize;
            assert_eq!(globe[i + 3], 255, "imagery must cover the comparison");
            differences.push(flat[i].abs_diff(globe[i]));
        }
    }
    let mean = differences.iter().map(|v| f64::from(*v)).sum::<f64>() / differences.len() as f64;
    differences.sort_unstable();
    let p95 = differences[differences.len() * 95 / 100];
    assert!(
        mean < 4.0 && p95 < 15,
        "globe raster error mean={mean}, p95={p95}"
    );
    assert!(
        flat.iter().step_by(4).max().expect("bright") - flat.iter().step_by(4).min().expect("dark")
            > 160,
        "exercise visible high-frequency imagery"
    );
}

async fn render(projection: &str) -> Vec<u8> {
    let mut map = prepared_map(projection).await;
    let n = 2_f64.powi(i32::from(TILE.2));
    let lon = (f64::from(TILE.0) + 0.5) / n * 360.0 - 180.0;
    let lat = (std::f64::consts::PI * (1.0 - 2.0 * (f64::from(TILE.1) + 0.5) / n))
        .sinh()
        .atan()
        .to_degrees();
    for tick in 0..12_u64 {
        map.run_xr_frame(XrFrame {
            timestamp: std::time::Duration::from_millis(tick * 16),
            opaque_environment: true,
            placement: ScenePlacement {
                anchor: ExternalAnchor {
                    position: LatLon::new(lat, lon),
                    altitude_meters: 0.0,
                },
                world_from_scene: Matrix4::identity(),
            },
            eyes: vec![XrEye {
                world_from_eye: Matrix4::from_translation(Vector3::new(0.0, 0.0, 130.0)),
                frustum: EyeFrustum::symmetric(Rad(1.0), 1.0, 10.0, 50_000_000.0),
                target: EyeTarget::default(),
            }],
            request_overscan: 1.0,
            prefetch: None,
        })
        .expect("frame");
    }
    let bytes = read_blocking(&map);
    if let Some(path) = std::env::var_os("MAPLIBRE_TEST_CAPTURE_DIR") {
        let path = std::path::PathBuf::from(path);
        std::fs::create_dir_all(&path).expect("capture directory");
        image::save_buffer(
            path.join(format!("imagery-{projection}.png")),
            &bytes,
            SIZE,
            SIZE,
            image::ColorType::Rgba8,
        )
        .expect("capture");
    }
    bytes
}

async fn prepared_map(projection: &str) -> HeadlessMap {
    let coords = WorldTileCoords::from((TILE.0, TILE.1, TILE.2.into()));
    let style: Style = serde_json::from_value(serde_json::json!({
        "version":8,"projection":{"type":projection},"terrain":{"source":"dem"},
        "sources":{"imagery":{"type":"raster","tiles":["offline://imagery"],"tileSize":512,"maxzoom":17},
        "dem":{"type":"raster-dem","tiles":["offline://dem"],"tileSize":256,"maxzoom":14,"encoding":"terrarium"}},
        "layers":[{"id":"imagery","type":"raster","source":"imagery"}]
    })).expect("style");
    let (kernel, renderer) = create_headless_renderer(SIZE, SIZE, None)
        .await
        .expect("renderer");
    let mut map = HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(RasterPlugin::<DefaultRasterTransferables>::default()),
            Box::new(TerrainPlugin::<DefaultDemTransferables>::default()),
            Box::new(
                HeadlessPlugin::new(false)
                    .preserve_tile_sources()
                    .retain_supplied_tiles(),
            ),
        ],
    )
    .expect("map");
    let image = RgbaImage::from_fn(512, 512, |x, y| {
        let v = (128.0 + 95.0 * (f64::from(x) * 0.35).sin() * (f64::from(y) * 0.35).cos()) as u8;
        Rgba([v, v, v, 255])
    });
    map.render_frames_with_terrain(
        ProcessedLayers::default(),
        vec![AvailableRasterLayerData {
            coords,
            source_layer: "imagery".into(),
            image,
        }],
        vec![(
            WorldTileCoords::from((TILE.0 / 8, TILE.1 / 8, 14_u8.into())),
            RgbaImage::from_pixel(256, 256, Rgba([128, 20, 0, 255])),
        )],
        3,
    )
    .expect("tiles");
    map
}

fn read_blocking(map: &HeadlessMap) -> Vec<u8> {
    let texture = map.head_texture().expect("color");
    let buffer = map.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("imagery precision regression"),
        size: u64::from(SIZE * SIZE * 4),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = map.device().create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::ImageCopyBuffer {
            buffer: &buffer,
            layout: wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(SIZE * 4),
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
