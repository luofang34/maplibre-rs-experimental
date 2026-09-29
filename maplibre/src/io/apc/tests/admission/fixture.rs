use std::{
    cell::{Cell, RefCell},
    future::Future,
    pin::Pin,
    rc::Rc,
};

use crate::{
    context::MapContext,
    coords::{WorldCoords, Zoom},
    environment::{Environment, OffscreenKernelConfig},
    headless::{create_headless_renderer, window::HeadlessMapWindowConfig},
    io::{
        apc::SchedulerAsyncProcedureCall,
        scheduler::{ScheduleError, Scheduler},
    },
    kernel::{Kernel, KernelBuilder},
    platform::{http_client::ReqwestHttpClient, ReqwestOffscreenKernelEnvironment},
    render::view_state::ViewState,
    style::Style,
    tcs::{system::System, tiles::TileComponent, world::World},
    window::PhysicalSize,
};

#[derive(Default)]
pub(super) struct Admission {
    pub reject: Cell<bool>,
    pub attempts: Cell<usize>,
    tasks: RefCell<Vec<Pin<Box<dyn Future<Output = ()>>>>>,
}

#[derive(Clone)]
pub(super) struct TestScheduler(Rc<Admission>);

impl Scheduler for TestScheduler {
    fn schedule<T>(&self, factory: impl FnOnce() -> T + Send + 'static) -> Result<(), ScheduleError>
    where
        T: Future<Output = ()> + 'static,
    {
        self.0.attempts.set(self.0.attempts.get().wrapping_add(1));
        if self.0.reject.get() {
            return Err(ScheduleError::Scheduling(Box::new(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "worker unavailable",
            ))));
        }
        self.0.tasks.borrow_mut().push(Box::pin(factory()));
        Ok(())
    }
}

pub(super) struct TestEnvironment;

impl Environment for TestEnvironment {
    type MapWindowConfig = HeadlessMapWindowConfig;
    type AsyncProcedureCall =
        SchedulerAsyncProcedureCall<ReqwestOffscreenKernelEnvironment, TestScheduler>;
    type Scheduler = TestScheduler;
    type HttpClient = ReqwestHttpClient;
    type OffscreenKernelEnvironment = ReqwestOffscreenKernelEnvironment;
}

#[derive(Clone, Copy)]
pub(super) enum Kind {
    Vector,
    Raster,
    Dem,
}

impl Kind {
    pub fn system(self, kernel: &Rc<Kernel<TestEnvironment>>) -> Box<dyn System> {
        match self {
            Self::Vector => Box::new(crate::vector::request_system::RequestSystem::<
                TestEnvironment,
                crate::vector::DefaultVectorTransferables,
            >::new(kernel)),
            Self::Raster => Box::new(crate::raster::request_system::RequestSystem::<
                TestEnvironment,
                crate::raster::DefaultRasterTransferables,
            >::new(kernel)),
            Self::Dem => Box::new(crate::terrain::request_system::RequestSystem::<
                TestEnvironment,
                crate::terrain::transferables::DefaultDemTransferables,
            >::new(kernel)),
        }
    }

    fn style(self, globe: bool) -> Style {
        let (source, layer) = match self {
            Self::Vector => ("vector", "fill"),
            Self::Raster => ("raster", "raster"),
            Self::Dem => ("raster-dem", "hillshade"),
        };
        let mut json = serde_json::json!({
            "version": 8,
            "sources": {"source": {"type": source, "tiles": ["https://example.invalid/{z}/{x}/{y}"], "maxzoom": 0}},
            "layers": [{"id": "layer", "type": layer, "source": "source"}],
        });
        if matches!(self, Self::Dem) {
            json["terrain"] = serde_json::json!({"source": "source"});
        }
        if globe {
            json["projection"] = serde_json::json!({"type": "globe"});
        }
        serde_json::from_value(json).expect("request style")
    }
}

pub(super) struct ExistingContent(pub u32);
impl TileComponent for ExistingContent {}

pub(super) async fn setup(
    kind: Kind,
    invalid_fov: bool,
) -> (Rc<Admission>, Box<dyn System>, MapContext) {
    let (_, renderer) = create_headless_renderer(16, 16, None)
        .await
        .expect("renderer");
    let size = PhysicalSize::new(16, 16).expect("size");
    let admission = Rc::new(Admission::default());
    admission.reject.set(true);
    let scheduler = TestScheduler(admission.clone());
    let kernel = Rc::new(
        KernelBuilder::new()
            .with_map_window_config(HeadlessMapWindowConfig::new(size))
            .with_http_client(ReqwestHttpClient::new::<String>(None))
            .with_scheduler(scheduler.clone())
            .with_apc(SchedulerAsyncProcedureCall::new(
                scheduler,
                OffscreenKernelConfig {
                    cache_directory: None,
                },
            ))
            .build()
            .expect("all kernel services configured"),
    );
    let mut world = World::default();
    world
        .tiles
        .spawn_mut(Default::default())
        .expect("tile")
        .insert(ExistingContent(42));
    let context = MapContext {
        renderer,
        style: kind.style(invalid_fov),
        world,
        view_state: ViewState::new(
            size,
            WorldCoords::from((256.0, 256.0)),
            Zoom::new(0.0),
            cgmath::Deg(0.0),
            cgmath::Rad(if invalid_fov { f64::NAN } else { 0.64 }),
        ),
    };
    (admission, kind.system(&kernel), context)
}
