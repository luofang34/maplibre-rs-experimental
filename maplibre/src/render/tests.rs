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
