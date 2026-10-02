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

#[tokio::test]
async fn both_polar_caps_export_surface_color_and_depth() {
    for latitude in [-89.999999, 89.999999] {
        let style: Style = serde_json::from_str(r##"{"version":8,"sources":{},"layers":[{"id":"background","type":"background","paint":{"background-color":"#718474"}}],"projection":{"type":"globe"}}"##).expect("style");
        let (kernel, renderer) = create_headless_renderer(64, 64, None)
            .await
            .expect("renderer");
        let mut map = HeadlessMap::new(
            style,
            renderer,
            kernel,
            vec![
                Box::new(RenderPlugin),
                Box::new(crate::background::BackgroundPlugin),
            ],
        )
        .expect("map");
        let depth = map.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("polar compositor depth"),
            size: wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        map.run_xr_frame(XrFrame {
            opaque_environment: false,
            timestamp: Duration::ZERO,
            placement: ScenePlacement {
                anchor: ExternalAnchor {
                    position: LatLon::new(latitude, 11.0),
                    altitude_meters: 0.0,
                },
                world_from_scene: Matrix4::identity(),
            },
            eyes: vec![XrEye {
                world_from_eye: Matrix4::from_translation(Vector3::new(0.0, 0.0, 40_000_000.0)),
                frustum: EyeFrustum::symmetric(Rad(1.0), 1.0, 0.1, 1e10),
                target: EyeTarget {
                    color: None,
                    depth: Some(depth.create_view(&Default::default())),
                },
            }],
            request_overscan: 1.0,
            prefetch: None,
        })
        .expect("polar frame");
        let colors = read_blocking(&map, map.head_texture().expect("color"));
        let depths = read_blocking(&map, &depth);
        for y in 31..33 {
            for x in 31..33 {
                let offset = (y * 64 + x) * 4;
                assert_eq!(colors[offset + 3], 255, "polar cap missing at {latitude}");
                let depth =
                    f32::from_le_bytes(depths[offset..offset + 4].try_into().expect("depth"));
                assert!(
                    depth > 0.0 && depth < 1.0,
                    "invalid polar depth {depth} at {latitude}"
                );
            }
        }
    }
}

fn read_blocking(map: &HeadlessMap, texture: &wgpu::Texture) -> Vec<u8> {
    let (width, height) = (texture.width(), texture.height());
    let buffer = map.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("polar readback"),
        size: u64::from(width * height * 4),
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
                bytes_per_row: Some(width * 4),
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
    let bytes = buffer
        .slice(..)
        .get_mapped_range()
        .expect("mapped readback range")
        .to_vec();
    buffer.unmap();
    bytes
}

fn clear_drape_white(map: &HeadlessMap, texture: &wgpu::Texture) {
    let mut encoder = map.device().create_command_encoder(&Default::default());
    for level in 0..texture.mip_level_count() {
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            base_mip_level: level,
            mip_level_count: Some(1),
            ..Default::default()
        });
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            multiview_mask: None,
            label: Some("polar test drape"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                depth_slice: None,
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        drop(pass);
    }
    map.queue().submit([encoder.finish()]);
}

#[path = "caps.rs"]
mod caps;
