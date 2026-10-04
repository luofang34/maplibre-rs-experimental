//! A headless map of OpenMapTiles-style roads whose shields roadshield draws on request,
//! shared by the example and the end-to-end test.

use std::{error::Error, path::Path, sync::Arc};

use geozero::mvt::{tile, Message as _};
use maplibre::{
    headless::{create_headless_renderer_with_loader, map::HeadlessMap},
    io::{
        resource_loader::SharedLoader,
        source_client::{HttpClient, SourceFetchError},
    },
    style::Style,
};
use maplibre_roadshield::{
    load_pack_dir_blocking, openmaptiles_shield_image, RoadShieldProvider, ShieldDisplay, NAMESPACE,
};

/// The side of the square image, in device pixels.
pub const SIZE: u32 = 512;

/// The routes the tile's three roads carry, as OpenMapTiles' `transportation_name` has them:
/// an interstate by its network class, a county route by its full route relation, and a state
/// route whose class names no state.
pub const ROADS: [(&str, &str, &str); 3] = [
    ("us-interstate", "287", ""),
    ("us-state", "609", "US:NJ:CR=609"),
    ("us-state", "609", ""),
];

/// Serves the same vector tile for every URL.
#[derive(Clone)]
struct Tiles(Arc<Vec<u8>>);

#[async_trait::async_trait]
impl HttpClient for Tiles {
    async fn fetch(&self, _url: &str) -> Result<Vec<u8>, SourceFetchError> {
        Ok(self.0.as_ref().clone())
    }
}

/// Three horizontal roads across the tile, each drawn and labelled. The view is centred on
/// the middle of the zoom-14 tile at Piscataway, New Jersey.
fn tile() -> Vec<u8> {
    let string = |value: &str| tile::Value {
        string_value: Some(value.to_owned()),
        ..Default::default()
    };
    let mut values = Vec::new();
    let mut features = Vec::new();
    for (index, (network, reference, route)) in ROADS.iter().enumerate() {
        let mut tags = Vec::new();
        for (key, value) in [(0, network), (1, reference), (2, route)] {
            if value.is_empty() {
                continue;
            }
            tags.extend([key, values.len() as u32]);
            values.push(string(value));
        }
        // A fifth of the tile apart around its middle, all within the view of its centre.
        let y = 1229 + 819 * index as u32;
        features.push(tile::Feature {
            r#type: Some(tile::GeomType::Linestring as i32),
            tags,
            // From (0, y) to (4096, y), zigzag encoded.
            geometry: vec![9, 0, y * 2, 10, 8192, 0],
            ..Default::default()
        });
    }
    let layer = |name: &str| tile::Layer {
        version: 2,
        name: name.into(),
        extent: Some(4096),
        keys: vec!["network".into(), "ref".into(), "route_1".into()],
        values: values.clone(),
        features: features.clone(),
    };
    geozero::mvt::Tile {
        layers: vec![layer("transportation"), layer("transportation_name")],
    }
    .encode_to_vec()
}

/// Roads with their shields, which carry the route number, so the layer draws no text.
pub fn style() -> Result<Style, serde_json::Error> {
    serde_json::from_value(serde_json::json!({
        "version":8,"center":[-74.454345703125,40.53885152535466],"zoom":14,
        "sources":{"openmaptiles":{"type":"vector",
            "tiles":["https://tiles.example/{z}/{x}/{y}.pbf"],"maxzoom":14}},
        "layers":[
            {"id":"background","type":"background","paint":{"background-color":"#f2efe9"}},
            {"id":"road","type":"line","source":"openmaptiles","source-layer":"transportation",
                "paint":{"line-color":"#b0b0b0","line-width":6}},
            {"id":"road-shield","type":"symbol","source":"openmaptiles",
                "source-layer":"transportation_name",
                "layout":{"symbol-placement":"line-center",
                    "icon-image":openmaptiles_shield_image(),
                    "icon-rotation-alignment":"viewport","text-field":""}}
        ]
    }))
}

/// A map at `pixel_ratio` whose shields `provider` draws.
pub async fn map(
    provider: RoadShieldProvider,
    pixel_ratio: f64,
) -> Result<HeadlessMap, Box<dyn Error>> {
    let (kernel, renderer) = create_headless_renderer_with_loader(
        SIZE,
        SIZE,
        Default::default(),
        SharedLoader::new(Tiles(Arc::new(tile()))),
    )
    .await?;
    let mut map = HeadlessMap::new(
        style()?,
        renderer,
        kernel,
        vec![
            Box::new(maplibre::render::RenderPlugin),
            Box::new(maplibre::background::BackgroundPlugin),
            Box::new(maplibre::vector::VectorPlugin::<
                maplibre::vector::DefaultVectorTransferables,
            >::default()),
            Box::new(maplibre::sdf::SdfPlugin::<
                maplibre::vector::DefaultVectorTransferables,
            >::default()),
        ],
    )?;
    map.set_pixel_ratio(pixel_ratio);
    map.image_providers()
        .ok_or("this map's workers do not share its image providers")?
        .register(NAMESPACE, Arc::new(provider));
    Ok(map)
}

/// The provider for the pack in `pack_dir`.
pub fn provider(pack_dir: &Path) -> Result<RoadShieldProvider, Box<dyn Error>> {
    Ok(RoadShieldProvider::new(
        load_pack_dir_blocking(pack_dir)?,
        ShieldDisplay::default(),
    )?)
}

/// Draws frames until nothing is loading or being made, then returns the frame's RGBA pixels.
pub async fn settle(map: &mut HeadlessMap) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut quiet = 0;
    for _ in 0..2000 {
        map.frame_input_mut().timestamp += std::time::Duration::from_millis(16);
        map.run_frame()?;
        tokio::task::yield_now().await;
        let busy = map.image_providers().map_or(0, |providers| {
            let stats = providers.stats();
            stats.running + stats.waiting
        });
        quiet = if busy == 0 && !map.needs_redraw() {
            quiet + 1
        } else {
            0
        };
        if quiet >= 20 {
            return read(map);
        }
    }
    Err("the map never settled".into())
}

/// The RGBA pixels of the last frame.
fn read(map: &HeadlessMap) -> Result<Vec<u8>, Box<dyn Error>> {
    let texture = map.head_texture().ok_or("the map has no colour target")?;
    let buffer = map.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("shield example pixels"),
        size: u64::from(SIZE * SIZE * 4),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = map
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
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
    let (sender, receiver) = std::sync::mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            sender.send(result).ok();
        });
    map.device().poll(wgpu::PollType::wait_indefinitely())?;
    receiver.recv()??;
    let bytes = buffer.slice(..).get_mapped_range()?.to_vec();
    buffer.unmap();
    Ok(bytes)
}
