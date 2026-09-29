use super::Event;
use maplibre::{
    environment::OffscreenKernelConfig,
    io::apc::SchedulerAsyncProcedureCall,
    kernel::{Kernel, KernelBuilder},
    map::Map,
    platform::{
        http_client::ReqwestHttpClient, scheduler::TokioScheduler,
        ReqwestOffscreenKernelEnvironment,
    },
    plugin::Plugin,
    render::{builder::RendererBuilder, graph::RenderGraph, resource::Head},
    schedule::{Schedule, Stage, StageResult},
    tcs::world::World,
};
use maplibre_winit::{WinitEnvironment, WinitMapWindowConfig};
use std::{
    cell::Cell,
    io::{Read, Write},
    net::TcpListener,
    rc::Rc,
    sync::mpsc,
    thread,
};
use winit::event_loop::EventLoopProxy;

type Environment = WinitEnvironment<
    TokioScheduler,
    ReqwestHttpClient,
    ReqwestOffscreenKernelEnvironment,
    SchedulerAsyncProcedureCall<ReqwestOffscreenKernelEnvironment, TokioScheduler>,
    Event,
>;

pub(super) fn metadata_server(
    proxy: EventLoopProxy<Event>,
) -> (String, mpsc::Sender<()>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("ephemeral metadata server");
    let url = format!(
        "http://{}/metadata.json",
        listener.local_addr().expect("bound address")
    );
    let (release, gate) = mpsc::channel();
    let server = thread::spawn(move || {
        for request in 0..2 {
            let (mut stream, _) = listener.accept().expect("metadata connection");
            let mut headers = Vec::new();
            while !headers.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream
                    .read_exact(&mut byte)
                    .expect("complete request headers");
                headers.push(byte[0]);
            }
            if request == 0 {
                proxy
                    .send_event(Event::MetadataBlocked)
                    .expect("notify pending initialization");
                gate.recv().expect("suspended callback releases response");
            }
            let body = br#"{"tiles":[]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            if let Err(error) = stream
                .write_all(response.as_bytes())
                .and_then(|()| stream.write_all(body))
            {
                assert!(
                    request == 0
                        && matches!(
                            error.kind(),
                            std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
                        ),
                    "HTTP response failed: {error}"
                );
            }
            if request == 0 {
                proxy
                    .send_event(Event::CanceledResponse)
                    .expect("notify late response");
            }
        }
    });
    (url, release, server)
}

pub(super) fn create_map(
    config: WinitMapWindowConfig<Event>,
    url: &str,
    proxy: EventLoopProxy<Event>,
) -> Map<Environment> {
    let kernel = KernelBuilder::new()
        .with_map_window_config(config)
        .with_http_client(ReqwestHttpClient::new::<String>(None))
        .with_scheduler(TokioScheduler::new())
        .with_apc(SchedulerAsyncProcedureCall::new(
            TokioScheduler::new(),
            OffscreenKernelConfig {
                cache_directory: None,
            },
        ))
        .build()
        .expect("kernel services");
    let style = serde_json::from_value(serde_json::json!({
        "version": 8, "sources": {"metadata": {"type":"vector", "url": url}}, "layers": []
    }))
    .expect("metadata style");
    Map::new(
        style,
        kernel,
        RendererBuilder::new(),
        vec![Box::new(PresentPlugin {
            proxy,
            generation: Cell::new(0),
        })],
    )
    .expect("bound window")
}

struct PresentPlugin {
    proxy: EventLoopProxy<Event>,
    generation: Cell<u64>,
}
impl Plugin<Environment> for PresentPlugin {
    fn build(
        &self,
        schedule: &mut Schedule,
        _: Rc<Kernel<Environment>>,
        _: &mut World,
        _: &mut RenderGraph,
    ) {
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        schedule.add_stage(
            "present",
            Present {
                proxy: self.proxy.clone(),
                generation,
            },
        );
    }
}
struct Present {
    proxy: EventLoopProxy<Event>,
    generation: u64,
}
impl Stage for Present {
    fn run(&mut self, context: &mut maplibre::context::MapContext) -> StageResult {
        let view = context.view_state.clone();
        let size = context.renderer.resources.surface.size();
        context.resize(maplibre::window::PhysicalSize::MIN, 2.0);
        assert_eq!(context.view_state.width(), 1.0);
        assert_eq!(context.view_state.height(), 1.0);
        context.view_state = view;
        context.renderer.resize_surface(size);
        let renderer = &mut context.renderer;
        renderer.resources.surface.reconfigure(&renderer.device);
        let Head::Headed(head) = renderer.resources.surface.head() else {
            panic!("headed surface")
        };
        let frame = match head.surface().get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            other => panic!("surface acquisition failed: {other:?}"),
        };
        assert_eq!(
            frame.texture.width(),
            size.width(),
            "configured GPU texture width"
        );
        assert_eq!(
            frame.texture.height(),
            size.height(),
            "configured GPU texture height"
        );
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = renderer.device.create_command_encoder(&Default::default());
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::GREEN),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
        }
        renderer.queue.submit([encoder.finish()]);
        drop(view);
        renderer.queue.present(frame);
        let size = renderer.resources.surface.size();
        self.proxy
            .send_event(Event::Presented {
                generation: self.generation,
                size: (size.width(), size.height()),
            })
            .expect("notify actual frame");
        Ok(())
    }
}
