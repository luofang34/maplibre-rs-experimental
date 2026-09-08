#![allow(clippy::expect_used, clippy::panic)]
use super::regression::frame;
use crate::{
    headless::{create_headless_renderer, map::HeadlessMap},
    render::{
        eventually::{Eventually, Eventually::Initialized},
        RenderPlugin,
    },
    style::Style,
    terrain::resources::TerrainResources,
};
use cgmath::{Deg, Matrix4};

const SIZE: u32 = 1024;

fn read_back_blocking(map: &HeadlessMap) -> Vec<u8> {
    texture_bytes_blocking(map, map.head_texture().expect("head texture"))
}

fn texture_bytes_blocking(map: &HeadlessMap, texture: &wgpu::Texture) -> Vec<u8> {
    let device = map.device();
    let queue = map.queue();
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("terrain regression readback"),
        size: u64::from(SIZE * SIZE * 4),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
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
    buffer.slice(..).map_async(wgpu::MapMode::Read, |result| {
        result.expect("readback mapped")
    });
    device.poll(wgpu::Maintain::Wait);
    let bytes = buffer.slice(..).get_mapped_range().to_vec();
    buffer.unmap();
    bytes
}

#[tokio::test]
async fn delayed_textures_preserve_surface_and_radial_haze_across_the_horizon() {
    let mut map = map().await;
    for pitch in [89.9, 90.1] {
        let view = || {
            let mut view = frame(16);
            view.eyes.truncate(1);
            view.eyes[0].world_from_eye =
                view.eyes[0].world_from_eye * Matrix4::from_angle_x(Deg(pitch));
            view
        };
        map.run_xr_frame(view()).expect("first frame");
        let first = read_back_blocking(&map);
        let Some(Initialized(terrain)) =
            map.world().resources.get::<Eventually<TerrainResources>>()
        else {
            panic!("terrain")
        };
        assert!(!terrain.draws().is_empty());
        if pitch < 90.0 {
            assert!(
                terrain
                    .draws()
                    .iter()
                    .any(|draw| terrain.drape_texture(draw.coords).is_none()),
                "exercise a surface whose texture has not been drawn"
            );
        }
        for _ in 0..16 {
            map.run_xr_frame(view()).expect("refinement");
        }
        let refined = read_back_blocking(&map);
        let difference = first
            .iter()
            .zip(&refined)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .expect("pixels");
        assert!(
            difference <= 2,
            "texture readiness changed surface pixels by {difference}"
        );
        // A flat surface's radial fog varies smoothly across tile boundaries near the horizon.
        for y in [SIZE * 54 / 100, SIZE * 60 / 100, SIZE * 90 / 100] {
            let row = &refined[(y * SIZE * 4) as usize..((y + 1) * SIZE * 4) as usize];
            let jump = row
                .windows(8)
                .step_by(4)
                .flat_map(|p| (0..3).map(move |c| p[c].abs_diff(p[c + 4])))
                .max()
                .expect("row");
            assert!(jump <= 4, "rectangular haze edge at row {y}: {jump}");
        }
        let at = |y: u32| refined[((y * SIZE + SIZE / 2) * 4) as usize];
        assert!(
            at(SIZE * 54 / 100) > at(SIZE * 90 / 100) + 10,
            "distant ground must actually receive fog"
        );
        capture_blocking(&refined, pitch);
    }
}

fn capture_blocking(bytes: &[u8], pitch: f64) {
    if let Some(path) = std::env::var_os("MAPLIBRE_TEST_CAPTURE_DIR") {
        let path = std::path::PathBuf::from(path);
        std::fs::create_dir_all(&path).expect("capture directory");
        image::save_buffer(
            path.join(format!("horizon-{pitch}.png")),
            bytes,
            SIZE,
            SIZE,
            image::ColorType::Rgba8,
        )
        .expect("capture");
    }
}

async fn map() -> HeadlessMap {
    let style: Style = serde_json::from_str(r##"{"version":8,"sources":{"dem":{"type":"raster-dem","tiles":["https://unused.example/{z}/{x}/{y}.png"]}},"layers":[{"id":"background","type":"background","paint":{"background-color":"#718474"}}],"terrain":{"source":"dem"},"sky":{"sky-color":"#223344","horizon-color":"#ccddee","fog-color":"#ccddee","fog-ground-blend":0.5,"horizon-fog-blend":0.8,"sky-horizon-blend":0.6},"projection":{"type":"mercator"}}"##).expect("style");
    let (kernel, renderer) = create_headless_renderer(SIZE, SIZE, None)
        .await
        .expect("renderer");
    HeadlessMap::new(
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
    .expect("map")
}

#[tokio::test]
async fn rolled_sky_exports_valid_compositor_depth_with_and_without_msaa() {
    for samples in [1, 4] {
        let mut map = map().await;
        map.map_context.renderer.settings.msaa = crate::render::settings::Msaa { samples };
        let depth = map.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("compositor sky depth"),
            size: wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let mut view = frame(16);
        view.eyes.truncate(1);
        view.eyes[0].world_from_eye = view.eyes[0].world_from_eye
            * Matrix4::from_angle_x(Deg(90.1))
            * Matrix4::from_angle_z(Deg(25.0));
        view.eyes[0].target.depth = Some(depth.create_view(&Default::default()));
        map.run_xr_frame(view).expect("rolled frame");
        let colors = read_back_blocking(&map);
        let depths = texture_bytes_blocking(&map, &depth);
        let horizon = map.view_state().horizon_line();
        let mut sky_pixels = 0u32;
        for y in 0..SIZE {
            for x in 0..SIZE {
                let pixel = cgmath::Point2::new(f64::from(x) + 0.5, f64::from(SIZE - y) - 0.5);
                if horizon.sky_distance(pixel) < 4.0 {
                    continue;
                }
                let offset = ((y * SIZE + x) * 4) as usize;
                let value =
                    f32::from_le_bytes(depths[offset..offset + 4].try_into().expect("depth"));
                assert!(colors[offset + 3] > 0, "sky has color");
                assert!(value.is_finite() && value > 0.0 && value < 1.0,
                    "opaque sky at ({x},{y}) has invalid compositor depth {value}, samples {samples}");
                sky_pixels = sky_pixels.wrapping_add(1);
            }
        }
        assert!(sky_pixels > SIZE * SIZE / 4);
    }
}

#[tokio::test]
async fn exposed_background_below_a_rolled_horizon_keeps_sky_coverage() {
    for samples in [1, 4] {
        let mut map = map().await;
        // Without a ground mesh, every pixel exposes the background a valley would reveal.
        map.map_context.style.terrain = None;
        map.map_context.renderer.settings.msaa = crate::render::settings::Msaa { samples };
        let mut view = frame(16);
        view.eyes.truncate(1);
        view.eyes[0].world_from_eye = view.eyes[0].world_from_eye
            * Matrix4::from_angle_x(Deg(100.0))
            * Matrix4::from_angle_z(Deg(25.0));
        map.run_xr_frame(view).expect("valley background");
        let pixels = read_back_blocking(&map);
        let horizon = map.view_state().horizon_line();
        let mut checked = 0_u32;
        for y in 0..SIZE {
            for x in 0..SIZE {
                let point = cgmath::Point2::new(f64::from(x), f64::from(SIZE - y));
                if horizon.sky_distance(point) >= -4.0 {
                    continue;
                }
                let pixel = &pixels[((y * SIZE + x) * 4) as usize..][..4];
                assert_eq!(pixel[3], 255, "sky coverage below horizon");
                assert!(
                    pixel[0] > 180 && pixel[1] > 180,
                    "exposed valley background must use the horizon color: {pixel:?}"
                );
                checked = checked.wrapping_add(1);
            }
        }
        assert!(checked > SIZE * SIZE / 10);
    }
}

mod ocean;

#[tokio::test]
async fn full_immersion_has_sky_coverage_even_before_the_globe_transition() {
    let mut map = map().await;
    map.map_context.style.projection = Some(crate::projection::ProjectionSpecification {
        projection_type: crate::projection::ProjectionType::Globe,
    });
    map.map_context.style.terrain = None;
    let mut view = frame(16);
    view.opaque_environment = true;
    view.eyes.truncate(1);
    view.eyes[0].world_from_eye =
        cgmath::Matrix4::from_translation(cgmath::Vector3::new(0.0, 0.0, 40_000_000.0))
            * Matrix4::from_angle_x(Deg(100.0));
    map.run_xr_frame(view).expect("opaque globe environment");
    let pixels = read_back_blocking(&map);
    assert!(
        pixels.chunks_exact(4).all(|pixel| pixel[3] == 255),
        "the host requests coverage across the whole frame"
    );
}
