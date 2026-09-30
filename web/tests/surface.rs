#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use maplibre::{
    background::BackgroundPlugin,
    environment::{OffscreenKernel, OffscreenKernelConfig},
    io::{
        apc::SchedulerAsyncProcedureCall,
        scheduler::{ScheduleError, Scheduler},
    },
    kernel::KernelBuilder,
    map::Map,
    projection::{ProjectionSpecification, ProjectionType},
    render::{
        builder::RendererBuilder, resource::Head, settings::WgpuSettings, RenderPlugin, Renderer,
    },
    sdf::SdfPlugin,
    vector::{DefaultVectorTransferables, VectorPlugin},
    window::{HeadedMapWindow, MapWindow, PhysicalSize},
};
use maplibre_winit::{WinitEnvironment, WinitMapWindowConfig};
use wasm_bindgen::JsCast;
use wasm_bindgen_test::*;
use web::WHATWGOffscreenKernelEnvironment;

wasm_bindgen_test_configure!(run_in_browser);

type TestEnvironment = WinitEnvironment<
    BrowserScheduler,
    <WHATWGOffscreenKernelEnvironment as OffscreenKernel>::HttpClient,
    WHATWGOffscreenKernelEnvironment,
    SchedulerAsyncProcedureCall<WHATWGOffscreenKernelEnvironment, BrowserScheduler>,
    (),
>;

struct BrowserScheduler;

impl Scheduler for BrowserScheduler {
    fn schedule<T>(&self, future: impl FnOnce() -> T + Send + 'static) -> Result<(), ScheduleError>
    where
        T: std::future::Future<Output = ()> + 'static,
    {
        wasm_bindgen_futures::spawn_local(future());
        Ok(())
    }
}

#[wasm_bindgen_test]
async fn configured_backend_renders_mercator_and_globe_frames() {
    let canvas = create_canvas();
    let backend = if cfg!(feature = "web-webgl") {
        wgpu::Backends::GL
    } else {
        wgpu::Backends::BROWSER_WEBGPU
    };
    let (config, window, stop, closed) = resumed_config().await;
    let mut map = create_map(backend, config);
    drop(window);
    map.initialize_renderer().await.expect("initialized map");
    let renderer = &map.context().expect("map context").renderer;
    assert!(backend.contains(renderer.adapter.get_info().backend.into()));
    let errors = renderer
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);
    for projection_type in [ProjectionType::Mercator, ProjectionType::Globe] {
        map.context_mut().expect("map context").style.projection =
            Some(ProjectionSpecification { projection_type });
        for _ in 0..2 {
            map.run_schedule().expect("map frame rendered");
            next_animation_frame().await;
        }
    }
    let error = errors.pop().await;
    assert!(error.is_none(), "GPU validation failed: {error:?}");
    assert_surface_retains_window(map, backend).await;
    canvas.remove();
    stop.send_event(()).expect("close surface fixture loop");
    wasm_bindgen_futures::JsFuture::from(closed)
        .await
        .expect("surface fixture exited");
}

fn create_canvas() -> web_sys::HtmlCanvasElement {
    let document = web_sys::window()
        .expect("browser window")
        .document()
        .expect("document");
    let canvas = document.create_element("canvas").expect("canvas");
    canvas.set_id("surface-test");
    let canvas = canvas
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .expect("canvas element");
    canvas.set_width(32);
    canvas.set_height(32);
    document
        .body()
        .expect("body")
        .append_child(&canvas)
        .expect("attached canvas");
    canvas
}

fn create_map(backend: wgpu::Backends, config: WinitMapWindowConfig<()>) -> Map<TestEnvironment> {
    let kernel = KernelBuilder::new()
        .with_map_window_config(config)
        .with_http_client(Default::default())
        .with_scheduler(BrowserScheduler)
        .with_apc(SchedulerAsyncProcedureCall::new(
            BrowserScheduler,
            OffscreenKernelConfig {
                cache_directory: None,
                ..Default::default()
            },
        ))
        .build()
        .expect("all kernel services configured");
    let style = serde_json::from_str(
        r##"{"version":8,"sources":{},"layers":[{"id":"background","type":"background",
             "paint":{"background-color":"#00ff00"}}]}"##,
    )
    .expect("background style");
    Map::new(
        style,
        kernel,
        RendererBuilder::new().with_wgpu_settings(WgpuSettings {
            backends: Some(backend),
            ..Default::default()
        }),
        vec![
            Box::new(RenderPlugin),
            Box::new(BackgroundPlugin),
            Box::<VectorPlugin<DefaultVectorTransferables>>::default(),
            Box::<SdfPlugin<DefaultVectorTransferables>>::default(),
        ],
    )
    .expect("map window")
}

async fn next_animation_frame() {
    let next_frame = js_sys::Promise::new(&mut |resolve, _reject| {
        web_sys::window()
            .expect("browser window")
            .request_animation_frame(&resolve)
            .expect("animation frame scheduled");
    });
    wasm_bindgen_futures::JsFuture::from(next_frame)
        .await
        .expect("animation frame");
}

async fn assert_surface_retains_window(map: Map<TestEnvironment>, backend: wgpu::Backends) {
    let owner = Arc::downgrade(map.window().handle());
    assert_eq!(
        owner.strong_count(),
        2,
        "map and surface each retain the window"
    );
    let mut renderer = RendererBuilder::new()
        .with_wgpu_settings(WgpuSettings {
            backends: Some(backend),
            ..Default::default()
        })
        .build()
        .initialize_renderer::<WinitMapWindowConfig<()>>(map.window())
        .await
        .expect("standalone renderer");
    assert_eq!(
        owner.strong_count(),
        3,
        "standalone surface retains its own owner"
    );
    let Head::Headed(head) = renderer.resources.surface.head_mut() else {
        panic!("expected window surface");
    };
    head.recreate_surface(map.window(), &renderer.instance)
        .expect("recreated surface");
    head.configure(&renderer.device);
    assert_eq!(
        owner.strong_count(),
        3,
        "recreation replaces one retained owner"
    );
    let error = head
        .recreate_surface(&UnavailableWindow(map.window().size()), &renderer.instance)
        .expect_err("unavailable handles cannot create a surface");
    assert!(
        std::error::Error::source(&error)
            .and_then(|source| source.downcast_ref::<wgpu::CreateSurfaceError>())
            .is_some(),
        "surface creation error retains its typed source"
    );
    assert_eq!(
        owner.strong_count(),
        3,
        "failed recreation preserves the surface owner"
    );
    drop(map);
    assert_eq!(
        owner.strong_count(),
        1,
        "standalone renderer outlives its host wrapper"
    );
    let errors = renderer
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);
    draw_retained_surface(&renderer);
    assert!(
        errors.pop().await.is_none(),
        "retained surface accepts a rendered frame"
    );
    drop(renderer);
    assert_eq!(
        owner.strong_count(),
        0,
        "dropping the last surface releases its owner"
    );
}

fn draw_retained_surface(renderer: &Renderer) {
    let Head::Headed(head) = renderer.resources.surface.head() else {
        panic!("expected window surface");
    };
    let frame = match head.surface().get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(frame)
        | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
        other => panic!("unable to acquire retained window frame: {other:?}"),
    };
    let view = frame
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = renderer
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
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
}

#[derive(Clone)]
struct UnavailableWindow(PhysicalSize);

impl wgpu::rwh::HasWindowHandle for UnavailableWindow {
    fn window_handle(&self) -> Result<wgpu::rwh::WindowHandle<'_>, wgpu::rwh::HandleError> {
        Err(wgpu::rwh::HandleError::Unavailable)
    }
}

impl wgpu::rwh::HasDisplayHandle for UnavailableWindow {
    fn display_handle(&self) -> Result<wgpu::rwh::DisplayHandle<'_>, wgpu::rwh::HandleError> {
        Err(wgpu::rwh::HandleError::Unavailable)
    }
}

impl MapWindow for UnavailableWindow {
    fn size(&self) -> PhysicalSize {
        self.0
    }
}

impl HeadedMapWindow for UnavailableWindow {
    type WindowHandle = Self;
    fn handle(&self) -> &Self::WindowHandle {
        self
    }
    fn request_redraw(&self) {}
    fn scale_factor(&self) -> f64 {
        1.0
    }
    fn id(&self) -> u64 {
        0
    }
}

type BoundWindow = (WinitMapWindowConfig<()>, maplibre_winit::WinitMapWindow<()>);

async fn resumed_config() -> (
    WinitMapWindowConfig<()>,
    maplibre_winit::WinitMapWindow<()>,
    winit::event_loop::EventLoopProxy<()>,
    js_sys::Promise,
) {
    use std::{cell::RefCell, rc::Rc};

    use winit::{
        application::ApplicationHandler, event::WindowEvent, event_loop::ActiveEventLoop,
        platform::web::EventLoopExtWebSys, window::WindowId,
    };
    struct Setup {
        result: Rc<RefCell<Option<BoundWindow>>>,
        resolve: js_sys::Function,
        closed: js_sys::Function,
        initialized: bool,
    }
    impl ApplicationHandler for Setup {
        fn resumed(&mut self, active: &ActiveEventLoop) {
            if self.initialized {
                return;
            }
            let config = WinitMapWindowConfig::new("surface-test".into());
            let window = config.create_window(active).expect("resumed canvas");
            *self.result.borrow_mut() = Some((config.with_window(&window), window));
            self.initialized = true;
            self.resolve
                .call0(&wasm_bindgen::JsValue::NULL)
                .expect("setup signaled");
        }
        fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
        fn user_event(&mut self, active: &ActiveEventLoop, _: ()) {
            active.exit();
        }
        fn exiting(&mut self, _: &ActiveEventLoop) {
            self.closed
                .call0(&wasm_bindgen::JsValue::NULL)
                .expect("exit signaled");
        }
    }
    let result = Rc::new(RefCell::new(None));
    let event_loop = winit::event_loop::EventLoop::new().expect("surface fixture loop");
    let stop = event_loop.create_proxy();
    let mut exit_callback = None;
    let closed = js_sys::Promise::new(&mut |resolve, _reject| exit_callback = Some(resolve));
    let mut setup = Some((event_loop, exit_callback.expect("exit resolver")));
    let signal = js_sys::Promise::new(&mut |resolve, _reject| {
        let (event_loop, closed) = setup.take().expect("single setup");
        event_loop.spawn_app(Setup {
            result: result.clone(),
            resolve,
            closed,
            initialized: false,
        });
    });
    wasm_bindgen_futures::JsFuture::from(signal)
        .await
        .expect("resumed event");
    let (config, window) = result.borrow_mut().take().expect("resumed setup complete");
    (config, window, stop, closed)
}
