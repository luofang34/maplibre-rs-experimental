#![allow(clippy::expect_used, clippy::panic)]

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
use cgmath::{Matrix4, Rad, SquareMatrix, Vector3};
use std::time::Duration;

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
    let buffer = map.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("polar readback"),
        size: 64 * 64 * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = map.device().create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::ImageCopyBuffer {
            buffer: &buffer,
            layout: wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: None,
            },
        },
        texture.size(),
    );
    map.queue().submit([encoder.finish()]);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, |result| result.expect("readback"));
    map.device().poll(wgpu::Maintain::Wait);
    let bytes = buffer.slice(..).get_mapped_range().to_vec();
    buffer.unmap();
    bytes
}

#[tokio::test]
async fn terrain_caps_show_the_draped_map_instead_of_background() {
    use crate::{
        render::eventually::{Eventually, Eventually::Initialized},
        terrain::resources::TerrainResources,
    };
    for latitude in [-89.999999, 89.999999] {
        let style: Style = serde_json::from_str(r##"{"version":8,"sources":{"dem":{"type":"raster-dem","tiles":["https://dem.example/{z}/{x}/{y}.png"],"encoding":"terrarium"}},"layers":[{"id":"background","type":"background","paint":{"background-color":"#006600"}}],"terrain":{"source":"dem"},"projection":{"type":"globe"}}"##).expect("style");
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
                Box::new(crate::terrain::TerrainPlugin::<
                    crate::terrain::DefaultDemTransferables,
                >::default()),
            ],
        )
        .expect("map");
        let frame = |timestamp| XrFrame {
            opaque_environment: false,
            timestamp: Duration::from_millis(timestamp),
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
                target: EyeTarget::default(),
            }],
            request_overscan: 1.0,
            prefetch: None,
        };
        map.run_xr_frame(frame(0)).expect("allocate terrain");
        map.run_xr_frame(frame(16)).expect("settle terrain");
        let Some(Initialized(terrain)) =
            map.world().resources.get::<Eventually<TerrainResources>>()
        else {
            panic!("terrain");
        };
        assert!(!terrain.draws().is_empty());
        for draw in terrain.draws() {
            let texture = terrain.drape_texture(draw.coords).expect("drape");
            clear_drape_white(&map, &texture.texture);
        }
        map.run_xr_frame(frame(32)).expect("polar terrain");
        let colors = read_blocking(&map, map.head_texture().expect("color"));
        for y in 31..33 {
            for x in 31..33 {
                let offset = (y * 64 + x) * 4;
                assert!(colors[offset] > 220 && colors[offset+1] > 220 && colors[offset+2] > 220,
                "polar terrain must retain its white drape at {latitude}: {:?}; white pixels {}, redrawn {:?}, draws {:?}", &colors[offset..offset+4], colors.chunks_exact(4).filter(|p| p[0] > 220 && p[1] > 220 && p[2] > 220).count(), map.world().resources.get::<crate::terrain::DrapePhase>().map(|p| p.targets.len()), match map.world().resources.get::<Eventually<TerrainResources>>() { Some(Initialized(t)) => t.draws().iter().map(|d|d.coords).collect::<Vec<_>>(), _ => Vec::new() });
            }
        }
    }
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
            label: Some("polar test drape"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
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
