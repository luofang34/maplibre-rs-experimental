use super::*;
use crate::{
    background::BackgroundPlugin,
    headless::{create_headless_renderer_with_settings, map::HeadlessMap, HeadlessPlugin},
    render::{
        settings::{Msaa, RendererSettings},
        RenderPlugin,
    },
    style::Style,
};

const SIZE: u32 = 64;

async fn map(samples: u32) -> HeadlessMap {
    let style: Style = serde_json::from_value(serde_json::json!({
        "version": 8, "sources": {},
        "layers": [{"id": "bg", "type": "background", "paint": {"background-color": "#ff0000"}}]
    }))
    .expect("style");
    let settings = RendererSettings {
        msaa: Msaa { samples },
        ..Default::default()
    };
    let (kernel, renderer) = create_headless_renderer_with_settings(SIZE, SIZE, None, settings)
        .await
        .expect("renderer");
    HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(BackgroundPlugin),
            Box::new(OverlayPlugin),
            Box::new(HeadlessPlugin::new(false)),
        ],
    )
    .expect("map")
}

/// Two triangles covering `[x0, x1] × [y0, y1]` in `color`.
fn rect(mesh: &mut OverlayMesh, (x0, y0, x1, y1): (f32, f32, f32, f32), color: [f32; 4]) {
    let base = mesh.vertices.len() as u32;
    for position in [[x0, y0], [x1, y0], [x1, y1], [x0, y1]] {
        mesh.vertices.push(OverlayVertex { position, color });
    }
    mesh.indices
        .extend([0, 1, 2, 0, 2, 3].iter().map(|i| base + i));
}

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// A horizontal and a vertical line crossing at the centre, each a black bar 4 px wide on a
/// white casing 12 px wide, every casing listed before any line.
fn crossing() -> OverlayMesh {
    let mut mesh = OverlayMesh::default();
    rect(&mut mesh, (8.0, 26.0, 56.0, 38.0), WHITE);
    rect(&mut mesh, (26.0, 8.0, 38.0, 56.0), WHITE);
    rect(&mut mesh, (8.0, 30.0, 56.0, 34.0), BLACK);
    rect(&mut mesh, (30.0, 8.0, 34.0, 56.0), BLACK);
    mesh
}

/// The map's frame as RGBA bytes, row by row.
fn frame(map: &mut HeadlessMap) -> Vec<[u8; 4]> {
    map.run_frame().expect("frame");
    let texture = map.head_texture().expect("head texture");
    let row = (SIZE * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = map.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("overlay readback"),
        size: u64::from(row * SIZE),
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
                bytes_per_row: Some(row),
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
        .expect("GPU readback completes");
    let bytes = buffer.slice(..).get_mapped_range().expect("range").to_vec();
    buffer.unmap();
    (0..SIZE)
        .flat_map(|y| {
            let bytes = &bytes;
            (0..SIZE).map(move |x| {
                let at = (y * row + x * 4) as usize;
                [bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]
            })
        })
        .collect()
}

fn at(pixels: &[[u8; 4]], x: u32, y: u32) -> [u8; 4] {
    pixels[(y * SIZE + x) as usize]
}

fn is_black(p: [u8; 4]) -> bool {
    p[0] < 16 && p[1] < 16 && p[2] < 16
}

fn is_white(p: [u8; 4]) -> bool {
    p[0] > 240 && p[1] > 240 && p[2] > 240
}

fn is_red(p: [u8; 4]) -> bool {
    p[0] > 240 && p[1] < 16 && p[2] < 16
}

async fn check_crossing(samples: u32) {
    let mut map = map(samples).await;
    map.set_overlay(crossing());
    let pixels = frame(&mut map);
    // The map stays under and around the overlay.
    assert!(is_red(at(&pixels, 2, 2)), "{:?}", at(&pixels, 2, 2));
    // Both lines are black where they cross and where each passes through the other's
    // casing: no casing cuts a line.
    for (x, y) in [(32, 32), (28, 32), (36, 32), (32, 28), (32, 36)] {
        assert!(
            is_black(at(&pixels, x, y)),
            "({x}, {y}): {:?}",
            at(&pixels, x, y)
        );
    }
    // The casings frame the lines.
    for (x, y) in [(16, 27), (16, 37), (27, 16), (37, 16)] {
        assert!(
            is_white(at(&pixels, x, y)),
            "({x}, {y}): {:?}",
            at(&pixels, x, y)
        );
    }
}

#[tokio::test]
async fn a_crossing_drawn_casings_first_keeps_both_lines_whole() {
    check_crossing(1).await;
}

#[tokio::test]
async fn the_overlay_is_drawn_into_a_multisampled_frame() {
    check_crossing(4).await;
}

#[tokio::test]
async fn an_empty_overlay_leaves_the_map_as_drawn() {
    let mut map = map(1).await;
    map.set_overlay(crossing());
    assert!(is_black(at(&frame(&mut map), 32, 32)));
    map.set_overlay(OverlayMesh::default());
    let pixels = frame(&mut map);
    assert!(pixels.iter().all(|&p| is_red(p)), "the map is drawn alone");
}
