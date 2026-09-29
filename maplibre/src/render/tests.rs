#![allow(clippy::expect_used, clippy::panic)]

use crate::{
    tcs::world::World,
    window::{MapWindow, PhysicalSize},
};

pub struct HeadlessMapWindow {
    size: PhysicalSize,
}

impl MapWindow for HeadlessMapWindow {
    fn size(&self) -> PhysicalSize {
        self.size
    }
}

#[tokio::test]
async fn test_render() {
    use log::LevelFilter;

    use crate::render::{
        graph::RenderGraph, graph_runner::RenderGraphRunner, resource::Surface, RenderResources,
        RendererSettings,
    };

    let _ = env_logger::builder()
        .filter_level(LevelFilter::Trace)
        .is_test(true)
        .try_init();
    let graph = RenderGraph::default();

    let backends = wgpu::Backends::from_env().unwrap_or(wgpu::Backends::all());
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        flags: Default::default(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = wgpu::util::initialize_adapter_from_env_or_default(&instance, None)
        .await
        .expect("Unable to initialize adapter");

    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: None,
            required_features: wgpu::Features::default(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::default(),
            ..Default::default()
        })
        .await
        .expect("Unable to request device");

    let render_state = RenderResources::new(Surface::from_image(
        &device,
        &adapter,
        &HeadlessMapWindow {
            size: PhysicalSize::new(100, 100).expect("invalid headless map size"),
        },
        &RendererSettings::default(),
    ));

    let world = World::default();
    RenderGraphRunner::run(&graph, &device, &queue, &render_state, &world)
        .expect("failed to run graph runner");
}

fn limited_settings() -> super::settings::WgpuSettings {
    super::settings::WgpuSettings {
        limits: wgpu::Limits {
            max_texture_dimension_2d: 1024,
            max_buffer_size: 64 * 1024 * 1024,
            min_uniform_buffer_offset_alignment: 512,
            ..wgpu::Limits::downlevel_defaults()
        },
        ..Default::default()
    }
}

#[tokio::test]
async fn requested_device_limits_follow_the_renderer_configuration() {
    let instance = wgpu::Instance::default();
    let (_, device, _) = super::Renderer::request_device(
        &instance,
        &limited_settings(),
        &wgpu::RequestAdapterOptions::default(),
    )
    .await
    .expect("configured limits supported by the test adapter");
    let actual = device.limits();
    assert_eq!(actual.max_texture_dimension_2d, 1024);
    assert_eq!(actual.max_buffer_size, 64 * 1024 * 1024);
    assert_eq!(actual.min_uniform_buffer_offset_alignment, 512);
}

#[tokio::test]
async fn constrained_device_limits_only_reduce_requested_capabilities() {
    let settings = super::settings::WgpuSettings {
        constrained_limits: Some(wgpu::Limits {
            max_texture_dimension_2d: 512,
            min_uniform_buffer_offset_alignment: 1024,
            ..wgpu::Limits::default()
        }),
        ..limited_settings()
    };
    let instance = wgpu::Instance::default();
    let (_, device, _) = super::Renderer::request_device(
        &instance,
        &settings,
        &wgpu::RequestAdapterOptions::default(),
    )
    .await
    .expect("constrained limits supported by the test adapter");
    let actual = device.limits();
    assert_eq!(actual.max_texture_dimension_2d, 512);
    assert_eq!(actual.max_buffer_size, 64 * 1024 * 1024);
    assert_eq!(actual.min_uniform_buffer_offset_alignment, 1024);
}

#[tokio::test]
async fn unsupported_device_limits_return_a_device_request_error() {
    let settings = super::settings::WgpuSettings {
        limits: wgpu::Limits {
            max_texture_dimension_2d: u32::MAX,
            ..wgpu::Limits::downlevel_defaults()
        },
        ..Default::default()
    };
    let instance = wgpu::Instance::default();
    let result = super::Renderer::request_device(
        &instance,
        &settings,
        &wgpu::RequestAdapterOptions::default(),
    )
    .await;
    assert!(matches!(result, Err(super::RenderError::RequestDevice(_))));
}
