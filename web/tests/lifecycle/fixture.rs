use std::{
    cell::Cell,
    future::poll_fn,
    rc::Rc,
    sync::{Arc, Mutex, Weak},
    task::{Poll, Waker},
};

use maplibre::{
    environment::OffscreenKernelConfig,
    io::{
        apc::SchedulerAsyncProcedureCall,
        scheduler::{ScheduleError, Scheduler},
        source_client::{HttpClient, SourceFetchError},
    },
    kernel::{Kernel, KernelBuilder},
    map::Map,
    plugin::Plugin,
    render::{
        builder::RendererBuilder, graph::RenderGraph, resource::Head, settings::WgpuSettings,
    },
    schedule::{Schedule, Stage, StageResult},
    tcs::world::World,
};
use maplibre_winit::{RawWinitWindow, WinitEnvironment, WinitMapWindowConfig};
use web::WHATWGOffscreenKernelEnvironment;
use winit::event_loop::EventLoopProxy;

#[derive(Clone, Debug)]
pub(super) enum Event {
    Blocked,
    LateWake,
    Presented { generation: u64, size: (u32, u32) },
}
#[derive(Default)]
pub(super) struct Observed {
    pub window: Weak<RawWinitWindow>,
    pub constructed: u64,
    pub ready_suspended: bool,
    pub complete: bool,
}
struct Gate {
    open: bool,
    waker: Option<Waker>,
    proxy: Option<EventLoopProxy<Event>>,
}
#[derive(Clone)]
pub(super) struct GateClient(Arc<Mutex<Gate>>);
impl GateClient {
    pub(super) fn new(proxy: EventLoopProxy<Event>) -> Self {
        Self(Arc::new(Mutex::new(Gate {
            open: false,
            waker: None,
            proxy: Some(proxy),
        })))
    }
    pub(super) fn release(&self) {
        let (wake, proxy) = {
            let mut gate = self.0.lock().expect("gate lock");
            gate.open = true;
            (
                gate.waker.take().expect("metadata future was polled"),
                gate.proxy.clone().expect("lifecycle proxy"),
            )
        };
        wake.wake();
        proxy.send_event(Event::LateWake).expect("late wake event");
    }
}
#[async_trait::async_trait(?Send)]
impl HttpClient for GateClient {
    async fn fetch(&self, _url: &str) -> Result<Vec<u8>, SourceFetchError> {
        poll_fn(|cx| {
            let mut gate = self.0.lock().expect("gate lock");
            if gate.open {
                return Poll::Ready(Ok(br#"{"tiles":[]}"#.to_vec()));
            }
            if gate.waker.is_none() {
                gate.proxy
                    .as_ref()
                    .expect("lifecycle proxy")
                    .send_event(Event::Blocked)
                    .expect("metadata pending");
            }
            gate.waker = Some(cx.waker().clone());
            Poll::Pending
        })
        .await
    }
}

pub(super) struct BrowserScheduler;
impl Scheduler for BrowserScheduler {
    fn schedule<T>(&self, future: impl FnOnce() -> T + Send + 'static) -> Result<(), ScheduleError>
    where
        T: std::future::Future<Output = ()> + 'static,
    {
        wasm_bindgen_futures::spawn_local(future());
        Ok(())
    }
}
type Environment = WinitEnvironment<
    BrowserScheduler,
    GateClient,
    WHATWGOffscreenKernelEnvironment,
    SchedulerAsyncProcedureCall<WHATWGOffscreenKernelEnvironment, BrowserScheduler>,
    Event,
>;

pub(super) fn create_map(
    config: WinitMapWindowConfig<Event>,
    client: GateClient,
    proxy: EventLoopProxy<Event>,
) -> Map<Environment> {
    let plugins: Vec<Box<dyn Plugin<Environment>>> = vec![Box::new(PresentPlugin {
        proxy,
        generation: Cell::new(0),
    })];
    map_with_plugins(config, client, plugins)
}

pub(super) fn startup_map(
    config: WinitMapWindowConfig<Event>,
    ready: Rc<Cell<bool>>,
    exited: js_sys::Function,
) -> Map<Environment> {
    let client = GateClient(Arc::new(Mutex::new(Gate {
        open: true,
        waker: None,
        proxy: None,
    })));
    map_with_plugins(
        config,
        client,
        vec![Box::new(ReadyPlugin { ready, exited })],
    )
}

fn map_with_plugins(
    config: WinitMapWindowConfig<Event>,
    client: GateClient,
    plugins: Vec<Box<dyn Plugin<Environment>>>,
) -> Map<Environment> {
    let kernel = KernelBuilder::new()
        .with_map_window_config(config)
        .with_http_client(client)
        .with_scheduler(BrowserScheduler)
        .with_apc(SchedulerAsyncProcedureCall::new(
            BrowserScheduler,
            OffscreenKernelConfig {
                cache_directory: None,
                ..Default::default()
            },
        ))
        .build()
        .expect("kernel services");
    let style = serde_json::from_value(serde_json::json!({"version":8,"sources":{"gate":{"type":"vector","url":"https://fixture.invalid/metadata"}},"layers":[]})).expect("metadata style");
    let backend = if cfg!(feature = "web-webgl") {
        wgpu::Backends::GL
    } else {
        wgpu::Backends::BROWSER_WEBGPU
    };
    Map::new(
        style,
        kernel,
        RendererBuilder::new().with_wgpu_settings(WgpuSettings {
            backends: Some(backend),
            ..Default::default()
        }),
        plugins,
    )
    .expect("bound canvas")
}

struct ReadyPlugin {
    ready: Rc<Cell<bool>>,
    exited: js_sys::Function,
}
impl Plugin<Environment> for ReadyPlugin {
    fn build(
        &self,
        _: &mut Schedule,
        _: Rc<Kernel<Environment>>,
        _: &mut World,
        _: &mut RenderGraph,
    ) {
        self.ready.set(true);
    }
}

impl Drop for ReadyPlugin {
    fn drop(&mut self) {
        self.exited
            .call0(&wasm_bindgen::JsValue::NULL)
            .expect("host map dropped");
    }
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
            .expect("actual frame event");
        Ok(())
    }
}
