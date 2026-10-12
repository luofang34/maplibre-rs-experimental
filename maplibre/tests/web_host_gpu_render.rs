//! A headless map on the page's WebGPU device fetches tiles and draws them. Needs the network;
//! run with `CHROMEDRIVER=<path> cargo test -p maplibre --target
//! wasm32-unknown-unknown --features headless --test web_host_gpu_render`.
#![cfg(target_arch = "wasm32")]

use std::collections::HashSet;

use futures::channel::oneshot;
use maplibre::{
    background::BackgroundPlugin,
    headless::{
        create_headless_renderer_on_host_gpu,
        map::{resolve_tile_json_sources, HeadlessMap},
    },
    hillshade::HillshadePlugin,
    io::{source_client::HttpClient, tile_retry::RequestKind},
    platform::http_client::web_http_client,
    plugin::Plugin,
    raster::{DefaultRasterTransferables, RasterPlugin},
    render::{frame_signals::ResourceReady, host_gpu::HostGpu, RenderPlugin},
    style::Style,
    terrain::{DefaultDemTransferables, TerrainPlugin},
    vector::{DefaultVectorTransferables, VectorPlugin},
};
use wasm_bindgen::{closure::Closure, JsCast};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_browser);

const SIZE: u32 = 256;
const STYLE_URL: &str = "https://tiles.openfreemap.org/styles/liberty";

async fn page_gpu() -> HostGpu {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::BROWSER_WEBGPU,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .expect("the page has a WebGPU adapter");
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default())
        .await
        .expect("WebGPU device");
    HostGpu {
        instance,
        adapter,
        device,
        queue,
    }
}

/// Resolves on the page's next animation frame, when a host would draw again.
async fn next_animation_frame() {
    let (sender, receiver) = oneshot::channel::<()>();
    let callback = Closure::once_into_js(move || {
        sender.send(()).ok();
    });
    web_sys::window()
        .expect("window")
        .request_animation_frame(callback.unchecked_ref())
        .expect("requestAnimationFrame");
    receiver.await.expect("animation frame");
}

async fn read_pixels(map: &HeadlessMap) -> Vec<[u8; 4]> {
    let texture = map.head_texture().expect("the map draws into a texture");
    let bytes_per_row = SIZE * 4;
    let buffer = map.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(bytes_per_row * SIZE),
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
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: None,
            },
        },
        texture.size(),
    );
    map.queue().submit([encoder.finish()]);
    let (sender, receiver) = oneshot::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            sender.send(result).ok();
        });
    receiver.await.expect("map callback").expect("readback");
    let pixels = buffer
        .slice(..)
        .get_mapped_range()
        .expect("mapped range")
        .chunks_exact(4)
        .map(|pixel| [pixel[0], pixel[1], pixel[2], pixel[3]])
        .collect();
    pixels
}

/// Draws frames on the page's animation frames until the map has nothing left to load, and
/// returns how many it drew and the resources reported ready.
async fn settle(map: &mut HeadlessMap) -> (usize, Vec<ResourceReady>) {
    let mut frames = 0;
    let mut ready = Vec::new();
    while frames < 600 && (frames == 0 || map.needs_redraw()) {
        map.run_frame()
            .expect("a frame with tiles in flight or failed succeeds");
        ready.extend(map.take_ready_resources());
        frames += 1;
        next_animation_frame().await;
    }
    (frames, ready)
}

async fn openfreemap_map() -> HeadlessMap {
    let body = web_http_client()
        .fetch(STYLE_URL)
        .await
        .expect("the style downloads");
    let mut style: Style = serde_json::from_slice(&body).expect("the style parses");
    // Central Amsterdam: canals, the IJ and a dense street grid.
    style.center = Some([4.9, 52.372]);
    style.zoom = Some(13.0);

    let (kernel, renderer) = create_headless_renderer_on_host_gpu(
        SIZE,
        SIZE,
        page_gpu().await,
        Default::default(),
        None,
    )
    .expect("renderer");
    resolve_tile_json_sources(&mut style, kernel.source_client()).await;
    let plugins: Vec<Box<dyn Plugin<_>>> = vec![
        Box::new(RenderPlugin),
        Box::new(BackgroundPlugin),
        Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
        Box::new(RasterPlugin::<DefaultRasterTransferables>::default()),
    ];
    HeadlessMap::new(style, renderer, kernel, plugins).expect("map")
}

async fn colours(map: &HeadlessMap) -> usize {
    read_pixels(map)
        .await
        .into_iter()
        .collect::<HashSet<[u8; 4]>>()
        .len()
}

#[wasm_bindgen_test]
async fn openfreemap_tiles_reach_a_map_on_the_page_gpu() {
    let mut map = openfreemap_map().await;
    let (frames, _) = settle(&mut map).await;

    let colours = colours(&map).await;
    assert!(
        colours > 16,
        "roads and water draw over the background after {frames} frames: {colours} colours"
    );
}

#[wasm_bindgen_test]
async fn a_source_the_browser_refuses_is_reported_per_tile_without_failing_the_frame() {
    let mut style: Style = serde_json::from_value(serde_json::json!({
        "version": 8, "center": [4.9, 52.372], "zoom": 3,
        "sources": {"refused": {"type": "vector",
            "tiles": ["https://example.com/refused/{z}/{x}/{y}.pbf"]}},
        "layers": [
            {"id": "bg", "type": "background"},
            {"id": "refused", "type": "line", "source": "refused", "source-layer": "roads"}
        ]
    }))
    .expect("refused style");
    let (kernel, renderer) = create_headless_renderer_on_host_gpu(
        SIZE,
        SIZE,
        page_gpu().await,
        Default::default(),
        None,
    )
    .expect("renderer");
    resolve_tile_json_sources(&mut style, kernel.source_client()).await;
    let plugins: Vec<Box<dyn Plugin<_>>> = vec![
        Box::new(RenderPlugin),
        Box::new(BackgroundPlugin),
        Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
    ];
    let mut map = HeadlessMap::new(style, renderer, kernel, plugins).expect("map");
    let (frames, ready) = settle(&mut map).await;

    assert!(
        ready.iter().any(|resource| matches!(
            resource,
            ResourceReady::Tile {
                kind: RequestKind::Vector,
                loaded: false,
                ..
            }
        )),
        "the refused tiles are reported failed within {frames} frames: {ready:?}"
    );
}

#[wasm_bindgen_test]
async fn terrarium_elevation_shades_and_raises_a_map_on_the_page_gpu() {
    let mut style: Style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "center": [7.6586, 45.9763], "zoom": 11, "pitch": 60,
        "sources": {"dem": {"type": "raster-dem", "encoding": "terrarium", "tileSize": 256,
            "maxzoom": 14,
            "tiles": ["https://s3.amazonaws.com/elevation-tiles-prod/terrarium/{z}/{x}/{y}.png"]}},
        "terrain": {"source": "dem", "exaggeration": 1.5},
        "layers": [
            {"id": "bg", "type": "background", "paint": {"background-color": "#e0e0d0"}},
            {"id": "shade", "type": "hillshade", "source": "dem"}
        ]
    }))
    .expect("terrain style");
    let (kernel, renderer) = create_headless_renderer_on_host_gpu(
        SIZE,
        SIZE,
        page_gpu().await,
        Default::default(),
        None,
    )
    .expect("renderer");
    resolve_tile_json_sources(&mut style, kernel.source_client()).await;
    let plugins: Vec<Box<dyn Plugin<_>>> = vec![
        Box::new(RenderPlugin),
        Box::new(BackgroundPlugin),
        Box::new(RasterPlugin::<DefaultRasterTransferables>::default()),
        Box::new(TerrainPlugin::<DefaultDemTransferables>::default()),
        Box::new(HillshadePlugin),
    ];
    let mut map = HeadlessMap::new(style, renderer, kernel, plugins).expect("map");
    let (frames, ready) = settle(&mut map).await;

    assert!(
        ready.iter().any(|resource| matches!(
            resource,
            ResourceReady::Tile {
                kind: RequestKind::Dem,
                loaded: true,
                ..
            }
        )),
        "an elevation tile loads after {frames} frames"
    );
    let colours = colours(&map).await;
    assert!(
        colours > 16,
        "hillshade shades the Matterhorn after {frames} frames: {colours} colours"
    );
}
