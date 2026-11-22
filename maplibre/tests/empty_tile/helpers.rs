#![allow(clippy::expect_used, clippy::panic)]

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use flate2::{write::ZlibEncoder, Compression};
use geozero::mvt::{tile, Message as _, Tile};
use maplibre::{
    background::BackgroundPlugin,
    coords::WorldTileCoords,
    headless::{
        create_headless_renderer_with_loader,
        map::{
            readback::{read_blocking, FrameReadback, PixelLayout},
            HeadlessMap,
        },
    },
    io::{
        resource_loader::SharedLoader,
        source_client::{HttpClient, SourceFetchError},
    },
    plugin::Plugin,
    raster::{DefaultRasterTransferables, RasterPlugin},
    render::{
        settings::{Msaa, RendererSettings},
        RenderPlugin,
    },
    style::Style,
    vector::{DefaultVectorTransferables, VectorPlugin},
};

pub(crate) const SIZE: u32 = 256;

#[derive(Clone, Debug)]
pub(crate) enum MockResponse {
    Bytes(Vec<u8>),
    Status(u16),
}

type TileResponder = Box<dyn FnMut(&str) -> MockResponse + Send>;

#[derive(Clone)]
pub(crate) struct MockTileClient {
    handler: Arc<Mutex<TileResponder>>,
    requests: Arc<Mutex<HashMap<String, usize>>>,
}

impl MockTileClient {
    pub(crate) fn new(handler: impl FnMut(&str) -> MockResponse + Send + 'static) -> Self {
        Self {
            handler: Arc::new(Mutex::new(Box::new(handler))),
            requests: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(crate) fn set_handler(&self, handler: impl FnMut(&str) -> MockResponse + Send + 'static) {
        *self.handler.lock().expect("mock lock") = Box::new(handler);
    }

    pub(crate) fn request_count(&self, needle: &str) -> usize {
        self.requests
            .lock()
            .expect("req lock")
            .iter()
            .filter(|(url, _)| url.contains(needle))
            .map(|(_, count)| *count)
            .sum()
    }
}

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait::async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait::async_trait)]
impl HttpClient for MockTileClient {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        *self
            .requests
            .lock()
            .expect("req lock")
            .entry(url.to_owned())
            .or_insert(0) += 1;
        let response = (self.handler.lock().expect("mock lock"))(url);
        match response {
            MockResponse::Bytes(bytes) => Ok(bytes),
            MockResponse::Status(204) => Ok(Vec::new()),
            MockResponse::Status(404) => Err(SourceFetchError::not_found(url)),
            MockResponse::Status(status) => Err(SourceFetchError::http_response(
                url,
                status,
                std::io::Error::other(format!("HTTP {status}")),
            )),
        }
    }
}

pub(crate) fn full_tile_mvt(layer_name: &str) -> Vec<u8> {
    Tile {
        layers: vec![tile::Layer {
            version: 2,
            name: layer_name.to_owned(),
            features: vec![tile::Feature {
                id: Some(1),
                tags: Vec::new(),
                r#type: Some(tile::GeomType::Polygon as i32),
                // MoveTo(0,0), LineTo(+4096,0), LineTo(0,+4096), LineTo(-4096,0), ClosePath
                geometry: vec![9, 0, 0, 26, 8192, 0, 0, 8192, 8191, 0, 15],
            }],
            keys: Vec::new(),
            values: Vec::new(),
            extent: Some(4096),
        }],
    }
    .encode_to_vec()
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

fn push_png_chunk(out: &mut Vec<u8>, chunk_type: &[u8; 4], payload: &[u8]) {
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(chunk_type);
    out.extend_from_slice(payload);
    let mut crc_input = Vec::with_capacity(4 + payload.len());
    crc_input.extend_from_slice(chunk_type);
    crc_input.extend_from_slice(payload);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

pub(crate) fn encode_solid_rgba_png(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
    let mut png = Vec::from([137, 80, 78, 71, 13, 10, 26, 10]);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    push_png_chunk(&mut png, b"IHDR", &ihdr);

    let mut raw = Vec::with_capacity((1 + width as usize * 4) * height as usize);
    for _ in 0..height {
        raw.extend(std::iter::once(0).chain((0..width).flat_map(|_| rgba)));
    }
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    std::io::Write::write_all(&mut encoder, &raw).expect("zlib write");
    let compressed = encoder.finish().expect("zlib finish");
    push_png_chunk(&mut png, b"IDAT", &compressed);
    push_png_chunk(&mut png, b"IEND", &[]);
    png
}

pub(crate) fn pixel_at(map: &HeadlessMap, x: u32, y: u32) -> [u8; 4] {
    let bytes = read_blocking(
        map,
        map.head_texture().expect("head texture"),
        wgpu::TextureAspect::All,
    )
    .expect("readback");
    let idx = ((y * SIZE + x) * 4) as usize;
    [bytes[idx], bytes[idx + 1], bytes[idx + 2], bytes[idx + 3]]
}

pub(crate) fn pixel_with_readback(
    map: &HeadlessMap,
    readback: &mut FrameReadback,
    x: u32,
    y: u32,
) -> [u8; 4] {
    let mut bytes = vec![0u8; readback.frame_len(PixelLayout::Rgba8)];
    readback
        .read_into_blocking(
            map,
            map.head_texture().expect("head texture"),
            PixelLayout::Rgba8,
            &mut bytes,
        )
        .expect("readback");
    let idx = ((y * SIZE + x) * 4) as usize;
    [bytes[idx], bytes[idx + 1], bytes[idx + 2], bytes[idx + 3]]
}

pub(crate) fn vector_style(maxzoom: u8, url_template: &str) -> Style {
    serde_json::from_value(serde_json::json!({
        "version": 8,
        "center": [0.0, 0.0],
        "zoom": 1.0,
        "sources": {
            "vec": {
                "type": "vector",
                "tiles": [url_template],
                "minzoom": 0,
                "maxzoom": maxzoom
            }
        },
        "layers": [
            {
                "id": "bg",
                "type": "background",
                "paint": { "background-color": "#0000ff" }
            },
            {
                "id": "water",
                "type": "fill",
                "source": "vec",
                "source-layer": "water",
                "paint": { "fill-color": "#ff0000" }
            }
        ]
    }))
    .expect("vector style")
}

pub(crate) fn raster_style(url_template: &str) -> Style {
    serde_json::from_value(serde_json::json!({
        "version": 8,
        "center": [0.0, 0.0],
        "zoom": 0.0,
        "sources": {
            "raster_src": {
                "type": "raster",
                "tiles": [url_template],
                "tileSize": 512,
                "minzoom": 0,
                "maxzoom": 1
            }
        },
        "layers": [
            {
                "id": "bg",
                "type": "background",
                "paint": { "background-color": "#0000ff" }
            },
            {
                "id": "raster_layer",
                "type": "raster",
                "source": "raster_src",
                "paint": { "raster-fade-duration": 0 }
            }
        ]
    }))
    .expect("raster style")
}

pub(crate) fn z1_quadrants() -> [WorldTileCoords; 4] {
    [
        WorldTileCoords::from((0, 0, 1_u8.into())),
        WorldTileCoords::from((1, 0, 1_u8.into())),
        WorldTileCoords::from((0, 1, 1_u8.into())),
        WorldTileCoords::from((1, 1, 1_u8.into())),
    ]
}

pub(crate) async fn make_loader_map(
    style: Style,
    client: MockTileClient,
    use_raster: bool,
) -> HeadlessMap {
    let settings = RendererSettings {
        msaa: Msaa { samples: 1 },
        ..Default::default()
    };
    let (kernel, renderer) =
        create_headless_renderer_with_loader(SIZE, SIZE, settings, SharedLoader::new(client))
            .await
            .expect("renderer");
    let mut plugins: Vec<Box<dyn Plugin<_>>> =
        vec![Box::new(RenderPlugin), Box::new(BackgroundPlugin)];
    if use_raster {
        plugins.push(Box::new(
            RasterPlugin::<DefaultRasterTransferables>::default(),
        ));
    } else {
        plugins.push(Box::new(
            VectorPlugin::<DefaultVectorTransferables>::default(),
        ));
    }
    HeadlessMap::new(style, renderer, kernel, plugins).expect("HeadlessMap")
}

pub(crate) async fn pump_until(map: &mut HeadlessMap, mut done: impl FnMut(&HeadlessMap) -> bool) {
    for _ in 0..160 {
        map.frame_input_mut().advance(Duration::from_millis(16));
        map.run_frame().expect("run_frame");
        if done(map) {
            // One extra frame guarantees GPU uploads and draws settle cleanly.
            map.frame_input_mut().advance(Duration::from_millis(16));
            map.run_frame().expect("settle frame");
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for loader pipeline condition");
}
