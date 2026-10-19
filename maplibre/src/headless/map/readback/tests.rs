#![allow(clippy::expect_used, clippy::panic)]

use std::time::Duration;

use cgmath::{Matrix4, Rad, SquareMatrix, Vector3};

use super::*;
use crate::{
    coords::LatLon,
    headless::{create_headless_renderer_with_settings, map::HeadlessMap},
    render::{
        camera::EyeFrustum,
        settings::RendererSettings,
        view_state::ExternalAnchor,
        xr::{EyeTarget, ScenePlacement, XrEye, XrFrame},
        RenderPlugin,
    },
    style::Style,
};

/// Four bytes a pixel make 400-byte rows, which a copy pads to 512.
const WIDTH: u32 = 100;
const HEIGHT: u32 = 30;
const BACKGROUND: [u8; 4] = [255, 128, 0, 255];

async fn map(format: wgpu::TextureFormat) -> HeadlessMap {
    let style: Style = serde_json::from_str(
        r##"{"version":8,"sources":{},"layers":[{"id":"background","type":"background","paint":{"background-color":"#ff8000"}}]}"##,
    )
    .expect("a background style parses");
    let settings = RendererSettings {
        texture_format: Some(format),
        ..RendererSettings::default()
    };
    let (kernel, renderer) = create_headless_renderer_with_settings(WIDTH, HEIGHT, None, settings)
        .await
        .expect("a headless renderer");
    HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(crate::background::BackgroundPlugin),
        ],
    )
    .expect("a map")
}

fn target(map: &HeadlessMap, usage: wgpu::TextureUsages) -> wgpu::Texture {
    let format = map.head_texture().expect("head texture").format();
    map.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("eye colour"),
        size: wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | usage,
        view_formats: &[],
    })
}

/// One eye three kilometres up, looking straight down, into `color`.
fn look_down(map: &mut HeadlessMap, color: &wgpu::Texture) {
    map.run_xr_frame(XrFrame {
        opaque_environment: true,
        timestamp: Duration::from_millis(16),
        placement: ScenePlacement {
            anchor: ExternalAnchor {
                position: LatLon::new(40.577, -74.738),
                altitude_meters: 0.0,
            },
            world_from_scene: Matrix4::identity(),
        },
        eyes: vec![XrEye {
            world_from_eye: Matrix4::from_translation(Vector3::new(0.0, 0.0, 3000.0)),
            frustum: EyeFrustum::symmetric(
                Rad(1.0),
                f64::from(WIDTH) / f64::from(HEIGHT),
                1.0,
                1.0e6,
            ),
            target: EyeTarget {
                color: Some(color.create_view(&Default::default())),
                depth: None,
            },
        }],
        request_overscan: 1.0,
        prefetch: None,
    })
    .expect("the eye renders");
}

#[tokio::test]
async fn an_eye_target_reads_back_as_the_drawn_background_in_both_layouts() {
    for format in [
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Bgra8Unorm,
    ] {
        let mut map = map(format).await;
        let color = target(&map, wgpu::TextureUsages::COPY_SRC);
        look_down(&mut map, &color);
        let mut readback = FrameReadback::new(&map, WIDTH, HEIGHT);

        let mut rgba = vec![0; readback.frame_len(PixelLayout::Rgba8)];
        readback
            .read_into_blocking(&map, &color, PixelLayout::Rgba8, &mut rgba)
            .expect("RGBA readback");
        for (index, pixel) in rgba.as_chunks::<4>().0.iter().enumerate() {
            assert_eq!(*pixel, BACKGROUND, "{format:?} pixel {index}");
        }

        // A second read through the same staging buffer, longer output untouched past the frame.
        let mut rgb = vec![7; readback.frame_len(PixelLayout::Rgb8) + 5];
        readback
            .read_into_blocking(&map, &color, PixelLayout::Rgb8, &mut rgb)
            .expect("RGB readback");
        let (frame, tail) = rgb.split_at(readback.frame_len(PixelLayout::Rgb8));
        for (index, pixel) in frame.as_chunks::<3>().0.iter().enumerate() {
            assert_eq!(pixel, &BACKGROUND[..3], "{format:?} pixel {index}");
        }
        assert_eq!(tail, [7; 5]);
    }
}

/// Writes a texel whose bytes name its column and row, in the texture's own channel order.
fn write_pattern(map: &HeadlessMap, texture: &wgpu::Texture) {
    let bgra = texture.format() == wgpu::TextureFormat::Bgra8Unorm;
    let mut texels = Vec::new();
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let (r, g, b, a) = (x as u8, y as u8, (x + y) as u8, 200 - y as u8);
            texels.extend(if bgra { [b, g, r, a] } else { [r, g, b, a] });
        }
    }
    map.upload_queue().write_texture(
        texture.as_image_copy(),
        &texels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(WIDTH * 4),
            rows_per_image: None,
        },
        texture.size(),
    );
}

/// Drives an async read on a native device: the first poll submits the copy and requests the
/// mapping, the device poll delivers it, and the second poll finishes the read.
fn resolve<F: std::future::Future>(map: &HeadlessMap, future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    if let std::task::Poll::Ready(output) = future.as_mut().poll(&mut context) {
        return output;
    }
    map.device()
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("device poll");
    match future.as_mut().poll(&mut context) {
        std::task::Poll::Ready(output) => output,
        std::task::Poll::Pending => panic!("the device poll delivered no mapping"),
    }
}

#[tokio::test]
async fn padded_rows_unpack_in_place_and_blue_first_textures_come_out_red_first() {
    for format in [
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Bgra8Unorm,
    ] {
        let map = map(format).await;
        let texture = target(
            &map,
            wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
        );
        write_pattern(&map, &texture);
        let mut readback = FrameReadback::new(&map, WIDTH, HEIGHT);
        let mut rgba = vec![0; readback.frame_len(PixelLayout::Rgba8)];
        resolve(
            &map,
            readback.read_into(&map, &texture, PixelLayout::Rgba8, &mut rgba),
        )
        .expect("async RGBA readback");
        let mut rgb = vec![0; readback.frame_len(PixelLayout::Rgb8)];
        readback
            .read_into_blocking(&map, &texture, PixelLayout::Rgb8, &mut rgb)
            .expect("RGB readback");
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let index = (y * WIDTH + x) as usize;
                let expected = [x as u8, y as u8, (x + y) as u8, 200 - y as u8];
                assert_eq!(
                    rgba[index * 4..][..4],
                    expected,
                    "{format:?} RGBA ({x},{y})"
                );
                assert_eq!(
                    rgb[index * 3..][..3],
                    expected[..3],
                    "{format:?} RGB ({x},{y})"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_texture_the_copy_cannot_serve_is_refused_before_any_gpu_work() {
    let map = map(wgpu::TextureFormat::Rgba8Unorm).await;
    let mut readback = FrameReadback::new(&map, WIDTH, HEIGHT);
    let mut out = vec![0; readback.frame_len(PixelLayout::Rgba8)];

    let unreadable = target(&map, wgpu::TextureUsages::empty());
    assert!(matches!(
        readback.read_into_blocking(&map, &unreadable, PixelLayout::Rgba8, &mut out),
        Err(ReadbackError::Usage(_))
    ));

    let mut short = vec![0; readback.frame_len(PixelLayout::Rgb8) - 1];
    let readable = target(&map, wgpu::TextureUsages::COPY_SRC);
    assert!(matches!(
        readback.read_into_blocking(&map, &readable, PixelLayout::Rgb8, &mut short),
        Err(ReadbackError::BufferTooSmall { needed, provided })
            if needed == (WIDTH * HEIGHT * 3) as usize && provided == needed - 1
    ));

    let mut other = FrameReadback::new(&map, WIDTH + 1, HEIGHT);
    assert!(matches!(
        other.read_into_blocking(&map, &readable, PixelLayout::Rgba8, &mut out),
        Err(ReadbackError::Size { texture: (WIDTH, HEIGHT), readback: (w, HEIGHT) }) if w == WIDTH + 1
    ));

    let depth = map.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("depth"),
        size: readable.size(),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    assert!(matches!(
        readback.read_into_blocking(&map, &depth, PixelLayout::Rgba8, &mut out),
        Err(ReadbackError::Format(wgpu::TextureFormat::Depth32Float))
    ));

    // The refusals left the staging buffer unmapped, so a good read still succeeds.
    readback
        .read_into_blocking(&map, &readable, PixelLayout::Rgba8, &mut out)
        .expect("a readable texture reads after refusals");
}
