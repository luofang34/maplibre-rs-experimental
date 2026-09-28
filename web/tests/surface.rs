#![allow(clippy::expect_used, clippy::panic)]

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
    render::{builder::RendererBuilder, settings::WgpuSettings, RenderPlugin},
    sdf::SdfPlugin,
    vector::{DefaultVectorTransferables, VectorPlugin},
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
    let mut map = create_map(backend);
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
    drop(map);
    canvas.remove();
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

fn create_map(backend: wgpu::Backends) -> Map<TestEnvironment> {
    let kernel = KernelBuilder::new()
        .with_map_window_config(WinitMapWindowConfig::new("surface-test".into()))
        .with_http_client(Default::default())
        .with_scheduler(BrowserScheduler)
        .with_apc(SchedulerAsyncProcedureCall::new(
            BrowserScheduler,
            OffscreenKernelConfig {
                cache_directory: None,
            },
        ))
        .build();
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
