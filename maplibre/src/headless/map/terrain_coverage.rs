//! Offline terrain frames with controlled source-tile arrivals.
#![allow(clippy::expect_used, clippy::panic)]

use image::{Rgba, RgbaImage};

use super::{HeadlessMap, ProcessedLayers};
use crate::{
    coords::WorldTileCoords,
    headless::{create_headless_renderer, HeadlessPlugin},
    hillshade::HillshadePlugin,
    raster::{AvailableRasterLayerData, DefaultRasterTransferables, RasterPlugin},
    render::RenderPlugin,
    style::Style,
    terrain::{DefaultDemTransferables, TerrainPlugin},
};

const SIZE: u32 = 512;

mod faults;
mod reload;

fn target() -> WorldTileCoords {
    WorldTileCoords::from((2423, 1389, 12_u8.into()))
}

async fn prepared_map(relief: bool) -> HeadlessMap {
    prepared_layers(relief, false).await
}

fn coverage_style(relief: bool, overlay: bool) -> Style {
    let coords = target();
    let n = 2_f64.powi(i32::from(u8::from(coords.z)));
    let lon = (f64::from(coords.x) + 0.5) / n * 360.0 - 180.0;
    let lat = (std::f64::consts::PI * (1.0 - 2.0 * (f64::from(coords.y) + 0.5) / n))
        .sinh()
        .atan()
        .to_degrees();
    let layer = if relief {
        serde_json::json!({"id":"paint","type":"color-relief","source":"paint",
            "paint":{"color-relief-opacity":0.5,"color-relief-color":
                ["interpolate",["linear"],["elevation"],0,"#00ff00",100,"#0000ff"]}})
    } else {
        serde_json::json!({"id":"paint","type":"raster","source":"paint"})
    };
    let mut style: Style = serde_json::from_value(serde_json::json!({
        "version":8,"center":[lon,lat],"zoom":12.125,"terrain":{"source":"dem"},
        "sources":{
            "paint":{"type":if relief {"raster-dem"} else {"raster"},
                "tiles":["offline://paint"],"tileSize":256,"maxzoom":14,"encoding":"terrarium"},
            "dem":{"type":"raster-dem","tiles":["offline://dem"],
                "tileSize":256,"maxzoom":14,"encoding":"terrarium"}},
        "layers":[{"id":"background","type":"background",
            "paint":{"background-color":"#101010"}},layer]
    }))
    .expect("style");
    if overlay {
        let mut layer =
            serde_json::from_value::<crate::style::layer::StyleLayer>(serde_json::json!({
                "id":"overlay","type":"color-relief","source":"paint",
                "paint":{"color-relief-opacity":0.5,"color-relief-color":
                    ["interpolate",["linear"],["elevation"],0,"#ff0000",100,"#ff0000"]}
            }))
            .expect("overlay");
        layer.index = 2;
        style.layers.push(layer);
    }
    style
}

async fn prepared_layers(relief: bool, overlay: bool) -> HeadlessMap {
    let coords = target();
    let (kernel, renderer) = create_headless_renderer(SIZE, SIZE, None)
        .await
        .expect("renderer");
    let mut map = HeadlessMap::new(
        coverage_style(relief, overlay),
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(RasterPlugin::<DefaultRasterTransferables>::default()),
            Box::new(HillshadePlugin),
            Box::new(faults::FaultPlugin),
            Box::new(TerrainPlugin::<DefaultDemTransferables>::default()),
            Box::new(
                HeadlessPlugin::new(false)
                    .preserve_tile_sources()
                    .retain_supplied_tiles(),
            ),
        ],
    )
    .expect("map");
    map.render_frames_with_terrain(
        ProcessedLayers::default(),
        vec![tile(coords, relief, false)],
        vec![(
            coords,
            RgbaImage::from_pixel(256, 256, Rgba([128, 0, 0, 255])),
        )],
        3,
    )
    .expect("parent frame");
    let required = map
        .required_raster_tile_coords("paint")
        .expect("source covering");
    assert!(coords
        .get_children()
        .iter()
        .all(|child| required.contains(child)));
    map
}

fn tile(coords: WorldTileCoords, relief: bool, child: bool) -> AvailableRasterLayerData {
    let color = match (relief, child) {
        (true, false) => [128, 0, 0, 255],
        (true, true) => [128, 100, 0, 255],
        (false, false) => [0, 255, 0, 255],
        (false, true) => [0, 0, 255, 255],
    };
    AvailableRasterLayerData {
        coords,
        source: "paint".into(),
        image: RgbaImage::from_pixel(256, 256, Rgba(color)),
    }
}

fn read_blocking(map: &HeadlessMap, name: &str) -> Vec<u8> {
    let texture = map.head_texture().expect("head texture");
    let buffer = map.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("terrain coverage readback"),
        size: u64::from(SIZE * SIZE * 4),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = map.device().create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
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
    map.device()
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("GPU completes");
    let bytes = buffer
        .slice(..)
        .get_mapped_range()
        .expect("mapped range")
        .to_vec();
    buffer.unmap();
    if let Some(directory) = std::env::var_os("MAPLIBRE_TEST_CAPTURE_DIR") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).expect("capture directory");
        image::save_buffer(
            directory.join(format!("{name}.png")),
            &bytes,
            SIZE,
            SIZE,
            image::ColorType::Rgba8,
        )
        .expect("capture");
    }
    bytes
}

fn assert_color(bytes: &[u8], expected: [u8; 4]) {
    for y in 16..SIZE - 16 {
        for x in 16..SIZE - 16 {
            let offset = ((y * SIZE + x) * 4) as usize;
            let pixel = &bytes[offset..offset + 4];
            assert!(
                pixel.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 2),
                "coverage or alpha changed at ({x},{y}): {pixel:?}, expected {expected:?}"
            );
        }
    }
}

mod tests;

mod stencil;

mod source_identity;

mod alpha;

mod uniforms;
