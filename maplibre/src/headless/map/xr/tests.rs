#![allow(clippy::expect_used, clippy::panic)]

use std::time::Duration;

use cgmath::{Matrix4, Rad, SquareMatrix, Vector3};

use crate::{
    coords::LatLon,
    headless::{create_headless_renderer, map::HeadlessMap},
    render::{
        camera::EyeFrustum,
        view_state::ExternalAnchor,
        xr::{EyeTarget, ScenePlacement, XrEye, XrFrame},
        RenderPlugin,
    },
    style::Style,
};

const SIZE: u32 = 64;

fn texture(device: &wgpu::Device, format: wgpu::TextureFormat) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// Fills a target with a value no frame produces, so a frame that draws into it can be told
/// from one that does not.
fn fill(device: &wgpu::Device, queue: &wgpu::Queue, color: &wgpu::Texture, depth: &wgpu::Texture) {
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let color = color.create_view(&Default::default());
    let depth = depth.create_view(&Default::default());
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("fill colour"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &color,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLUE),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("fill depth"),
        color_attachments: &[],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: &depth,
            depth_ops: Some(wgpu::Operations {
                load: wgpu::LoadOp::Clear(1.0),
                store: wgpu::StoreOp::Store,
            }),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
    });
    queue.submit([encoder.finish()]);
}

fn read_back(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Vec<u8> {
    let bytes = u64::from(SIZE * SIZE * 4);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: bytes,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
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
    queue.submit([encoder.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |_| ());
    device.poll(wgpu::Maintain::Wait);
    let data = buffer.slice(..).get_mapped_range().to_vec();
    buffer.unmap();
    data
}

#[tokio::test]
async fn each_eye_draws_into_its_own_targets() {
    let style: Style = serde_json::from_str(r#"{"version": 8, "sources": {}, "layers": []}"#)
        .expect("an empty style parses");
    let (kernel, renderer) = create_headless_renderer(SIZE, SIZE, None)
        .await
        .expect("a headless renderer");
    let format = renderer.state().surface().surface_format();
    let mut map =
        HeadlessMap::new(style, renderer, kernel, vec![Box::new(RenderPlugin)]).expect("a map");
    let colors = [texture(map.device(), format), texture(map.device(), format)];
    let depths = [
        texture(map.device(), wgpu::TextureFormat::Depth32Float),
        texture(map.device(), wgpu::TextureFormat::Depth32Float),
    ];
    for (color, depth) in colors.iter().zip(&depths) {
        fill(map.device(), map.queue(), color, depth);
    }
    let frame = XrFrame {
        timestamp: Duration::from_millis(16),
        placement: ScenePlacement {
            anchor: ExternalAnchor {
                position: LatLon::new(47.0, 11.0),
                altitude_meters: 0.0,
            },
            world_from_scene: Matrix4::identity(),
        },
        eyes: (0..2)
            .map(|index| XrEye {
                // Two eyes a little apart, three kilometres up, looking straight down.
                world_from_eye: Matrix4::from_translation(Vector3::new(
                    f64::from(index) * 0.064,
                    0.0,
                    3000.0,
                )),
                frustum: EyeFrustum::symmetric(Rad(1.0), 1.0, 0.1, 100_000.0),
                target: EyeTarget {
                    color: Some(colors[index as usize].create_view(&Default::default())),
                    depth: Some(depths[index as usize].create_view(&Default::default())),
                },
            })
            .collect(),
        request_overscan: 1.0,
    };

    map.run_xr_frame(frame).expect("both eyes render");

    for (index, (color, depth)) in colors.iter().zip(&depths).enumerate() {
        let pixels = read_back(map.device(), map.queue(), color);
        assert!(
            pixels.iter().all(|&byte| byte == 0),
            "eye {index} was cleared by the frame, not left blue"
        );
        let depth = read_back(map.device(), map.queue(), depth);
        assert!(
            depth.iter().all(|&byte| byte == 0),
            "eye {index} received the frame's depth, which is zero at the far plane"
        );
    }
    let pose = map.view_state().camera_pose();
    assert!((pose.altitude_meters - 3000.0).abs() < 1e-6, "{pose:?}");
}
