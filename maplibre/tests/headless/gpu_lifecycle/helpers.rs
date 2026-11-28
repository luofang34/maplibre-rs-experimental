#![allow(clippy::expect_used, clippy::panic)]

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use geozero::mvt::{tile, Message as _, Tile};
use maplibre::{
    background::BackgroundPlugin,
    headless::{
        create_headless_renderer_with_loader, create_headless_renderer_with_settings,
        environment::HeadlessEnvironment,
        map::{
            readback::{FrameReadback, PixelLayout},
            HeadlessMap,
        },
        HeadlessPlugin,
    },
    io::{
        resource_loader::SharedLoader,
        source_client::{HttpClient, SourceFetchError},
    },
    map::TerrainPlugin,
    plugin::Plugin,
    raster::{DefaultRasterTransferables, RasterPlugin},
    render::settings::{Msaa, RendererSettings},
    sdf::SdfPlugin,
    style::Style,
    terrain::DefaultDemTransferables,
    vector::{DefaultVectorTransferables, VectorPlugin},
    RenderPlugin,
};

pub(crate) const SIZE: u32 = 256;

type TileHandler = Box<dyn FnMut(&str) -> Vec<u8> + Send>;

#[derive(Clone)]
pub(crate) struct MockServer {
    handler: Arc<Mutex<TileHandler>>,
}

impl MockServer {
    pub(crate) fn new(handler: impl FnMut(&str) -> Vec<u8> + Send + 'static) -> Self {
        Self {
            handler: Arc::new(Mutex::new(Box::new(handler))),
        }
    }
}

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait::async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait::async_trait)]
impl HttpClient for MockServer {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        Ok((self.handler.lock().expect("mock lock"))(url))
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct GpuCounts {
    pub(crate) buffers: usize,
    pub(crate) textures: usize,
    pub(crate) bind_groups: usize,
    pub(crate) samplers: usize,
    pub(crate) pipelines: usize,
}

pub(crate) fn gpu_counts(map: &mut HeadlessMap) -> GpuCounts {
    map.queue().submit([]);
    let _poll = map.device().poll(wgpu::PollType::wait_indefinitely());
    let report = map
        .context()
        .renderer
        .instance
        .generate_report()
        .expect("wgpu GlobalReport");
    let hub = report.hub_report();
    GpuCounts {
        buffers: hub.buffers.num_allocated,
        textures: hub.textures.num_allocated,
        bind_groups: hub.bind_groups.num_allocated,
        samplers: hub.samplers.num_allocated,
        pipelines: hub.render_pipelines.num_allocated,
    }
}

pub(crate) fn settle_frames(map: &mut HeadlessMap, frames: usize) {
    for _ in 0..frames {
        map.frame_input_mut().advance(Duration::from_millis(16));
        map.run_frame().expect("run frame");
        let _poll = map.device().poll(wgpu::PollType::wait_indefinitely());
    }
}

pub(crate) fn solid_rgb_png(width: u32, height: u32, r: u8, g: u8, b: u8) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png header");
    let row = [r, g, b].repeat(width as usize);
    let data = row.repeat(height as usize);
    writer.write_image_data(&data).expect("png data");
    drop(writer);
    out
}

pub(crate) fn minimal_mvt(layer_name: &str) -> Vec<u8> {
    let mut feature = tile::Feature::default();
    feature.set_type(tile::GeomType::Polygon);
    feature.geometry = vec![9, 0, 0, 26, 256, 0, 0, 256, 255, 0, 15];
    let layer = tile::Layer {
        name: layer_name.to_owned(),
        version: 2,
        extent: Some(4096),
        features: vec![feature],
        ..Default::default()
    };
    Tile {
        layers: vec![layer],
    }
    .encode_to_vec()
}

fn base_plugins(with_request_stage: bool) -> Vec<Box<dyn Plugin<HeadlessEnvironment>>> {
    let mut plugins: Vec<Box<dyn Plugin<HeadlessEnvironment>>> = vec![
        Box::new(RenderPlugin),
        Box::new(BackgroundPlugin),
        Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
        Box::new(SdfPlugin::<DefaultVectorTransferables>::default()),
        Box::new(RasterPlugin::<DefaultRasterTransferables>::default()),
        Box::new(TerrainPlugin::<DefaultDemTransferables>::default()),
    ];
    if !with_request_stage {
        plugins.push(Box::new(HeadlessPlugin::new(false)));
    }
    plugins
}

pub(crate) async fn make_map(
    style: Style,
    server: Option<MockServer>,
) -> (HeadlessMap, FrameReadback) {
    let settings = RendererSettings {
        msaa: Msaa { samples: 1 },
        ..Default::default()
    };
    let with_request_stage = server.is_some();
    let (kernel, renderer) = match server {
        Some(client) => {
            create_headless_renderer_with_loader(SIZE, SIZE, settings, SharedLoader::new(client))
                .await
                .expect("renderer")
        }
        None => create_headless_renderer_with_settings(SIZE, SIZE, None, settings)
            .await
            .expect("renderer"),
    };
    let map =
        HeadlessMap::new(style, renderer, kernel, base_plugins(with_request_stage)).expect("map");
    let readback = FrameReadback::new(&map, SIZE, SIZE);
    (map, readback)
}

pub(crate) fn capture_pixels(map: &HeadlessMap, readback: &mut FrameReadback) -> Vec<u8> {
    let mut buf = vec![0_u8; readback.frame_len(PixelLayout::Rgba8)];
    readback
        .read_into_blocking(
            map,
            map.head_texture().expect("head texture"),
            PixelLayout::Rgba8,
            &mut buf,
        )
        .expect("readback");
    buf
}

pub(crate) fn max_channel_diff(a: &[u8], b: &[u8]) -> u8 {
    a.iter()
        .zip(b)
        .map(|(&x, &y)| x.abs_diff(y))
        .max()
        .unwrap_or(0)
}

pub(crate) fn projection_style(globe: bool) -> Style {
    serde_json::from_value(serde_json::json!({
        "version": 8,
        "projection": {"type": if globe { "globe" } else { "mercator" }},
        "center": [0.0, 0.0],
        "zoom": 1.2,
        "sources": {
            "gj": {
                "type": "geojson",
                "data": {
                    "type": "FeatureCollection",
                    "features": [{
                        "type": "Feature",
                        "geometry": {
                            "type": "Polygon",
                            "coordinates": [[[-30.0, -30.0], [30.0, -30.0], [30.0, 30.0], [-30.0, 30.0], [-30.0, -30.0]]]
                        },
                        "properties": {}
                    }]
                }
            }
        },
        "layers": [
            {"id": "bg", "type": "background", "paint": {"background-color": "#102030"}},
            {"id": "land", "type": "fill", "source": "gj", "paint": {"fill-color": "#cc8844"}}
        ]
    }))
    .expect("style")
}
