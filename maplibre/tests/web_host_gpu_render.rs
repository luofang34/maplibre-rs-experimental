//! A headless map on the page's WebGPU device fetches OpenFreeMap tiles and draws them. Needs
//! the network; run with `CHROMEDRIVER=<path> cargo test -p maplibre --target
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
    io::source_client::HttpClient,
    platform::http_client::web_http_client,
    plugin::Plugin,
    raster::{DefaultRasterTransferables, RasterPlugin},
    render::{host_gpu::HostGpu, RenderPlugin},
    style::Style,
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

#[wasm_bindgen_test]
async fn openfreemap_tiles_reach_a_map_on_the_page_gpu() {
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
    let mut map = HeadlessMap::new(style, renderer, kernel, plugins).expect("map");

    let mut frames = 0;
    while frames < 600 && (frames == 0 || map.needs_redraw()) {
        map.run_frame()
            .expect("a frame with tiles in flight succeeds");
        frames += 1;
        next_animation_frame().await;
    }

    let colors: HashSet<[u8; 4]> = read_pixels(&map).await.into_iter().collect();
    assert!(
        colors.len() > 16,
        "roads and water draw over the background after {frames} frames: {} colours",
        colors.len()
    );
}
