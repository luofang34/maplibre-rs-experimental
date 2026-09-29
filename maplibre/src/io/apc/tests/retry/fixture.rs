use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
    time::Duration,
    vec::IntoIter,
};

use super::source::Source;
use crate::{
    context::MapContext,
    coords::{WorldCoords, Zoom},
    environment::{Environment, OffscreenKernel, OffscreenKernelConfig},
    headless::{create_headless_renderer, window::HeadlessMapWindowConfig},
    io::{
        apc::{
            AsyncProcedure, AsyncProcedureCall, CallError, Context, Input, IntoMessage, Message,
            SendError,
        },
        scheduler::NopScheduler,
    },
    kernel::{Kernel, KernelBuilder},
    platform::{http_client::ReqwestHttpClient, ReqwestOffscreenKernelEnvironment},
    raster::RasterLayersDataComponent,
    render::{frame_input::FrameInput, view_state::ViewState},
    style::Style,
    tcs::{system::System, world::World},
    terrain::DemTileComponent,
    window::PhysicalSize,
};

#[derive(Clone, Default)]
pub(super) struct Replies(Arc<Mutex<Vec<Message>>>);
impl Context for Replies {
    fn send_back<T: IntoMessage>(&self, message: T) -> Result<(), SendError> {
        self.0.lock().expect("replies").push(message.into());
        Ok(())
    }
}

type Procedure = AsyncProcedure<ReqwestOffscreenKernelEnvironment, Replies>;
#[derive(Default)]
pub(super) struct Calls {
    pending: RefCell<Vec<(Input, Procedure)>>,
    replies: Replies,
}
impl AsyncProcedureCall<ReqwestOffscreenKernelEnvironment> for Calls {
    type Context = Replies;
    type ReceiveIterator<F: FnMut(&Message) -> bool> = IntoIter<Message>;
    fn receive<F: FnMut(&Message) -> bool>(&self, mut filter: F) -> Self::ReceiveIterator<F> {
        self.replies
            .0
            .lock()
            .expect("replies")
            .extract_if(.., |message| filter(message))
            .collect::<Vec<_>>()
            .into_iter()
    }
    fn call(&self, input: Input, procedure: Procedure) -> Result<(), CallError> {
        self.pending.borrow_mut().push((input, procedure));
        Ok(())
    }
}
impl Calls {
    pub async fn complete(&self) {
        let work = std::mem::take(&mut *self.pending.borrow_mut());
        for (input, procedure) in work {
            procedure(
                input,
                self.replies.clone(),
                ReqwestOffscreenKernelEnvironment::create(OffscreenKernelConfig {
                    cache_directory: None,
                }),
            )
            .await
            .expect("worker completed");
        }
    }
    pub fn take_replies(&self) -> Vec<Message> {
        std::mem::take(&mut *self.replies.0.lock().expect("replies"))
    }
    pub fn deliver(&self, messages: Vec<Message>) {
        self.replies.0.lock().expect("replies").extend(messages);
    }
    pub fn pending(&self) -> usize {
        self.pending.borrow().len()
    }
}

pub(super) struct TestEnvironment;
impl Environment for TestEnvironment {
    type MapWindowConfig = HeadlessMapWindowConfig;
    type AsyncProcedureCall = Calls;
    type Scheduler = NopScheduler;
    type HttpClient = ReqwestHttpClient;
    type OffscreenKernelEnvironment = ReqwestOffscreenKernelEnvironment;
}

#[derive(Clone, Copy)]
pub(super) enum Kind {
    Raster,
    Dem,
}
impl Kind {
    fn style(self, url: &str, multiple: bool) -> Style {
        let source_type = if matches!(self, Self::Raster) {
            "raster"
        } else {
            "raster-dem"
        };
        let layer_type = if matches!(self, Self::Raster) {
            "raster"
        } else {
            "hillshade"
        };
        let mut json = serde_json::json!({"version":8,
            "sources":{"source":{"type":source_type,"tiles":[format!("{url}/unstable/{{z}}/{{x}}/{{y}}")],"maxzoom":0,"encoding":"terrarium"}},
            "layers":[{"id":"layer","source":"source","type":layer_type}]});
        if matches!(self, Self::Dem) {
            json["terrain"] = serde_json::json!({"source":"source"});
        }
        if multiple {
            json["sources"]["healthy"] = serde_json::json!({"type":"raster","tiles":[format!("{url}/healthy/{{z}}/{{x}}/{{y}}")],"maxzoom":0});
            json["layers"]
                .as_array_mut()
                .expect("layers")
                .push(serde_json::json!({"id":"healthy","source":"healthy","type":"raster"}));
        }
        serde_json::from_value(json).expect("style")
    }
    fn request(self, kernel: &Rc<Kernel<TestEnvironment>>) -> Box<dyn System> {
        match self {
            Self::Raster => Box::new(crate::raster::request_system::RequestSystem::<
                TestEnvironment,
                crate::raster::DefaultRasterTransferables,
            >::new(kernel)),
            Self::Dem => Box::new(crate::terrain::request_system::RequestSystem::<
                TestEnvironment,
                crate::terrain::DefaultDemTransferables,
            >::new(kernel)),
        }
    }
    fn populate(self, kernel: &Rc<Kernel<TestEnvironment>>) -> Box<dyn System> {
        match self {
            Self::Raster => Box::new(crate::raster::populate_world_system::PopulateWorldSystem::<
                TestEnvironment,
                crate::raster::DefaultRasterTransferables,
            >::new(kernel)),
            Self::Dem => Box::new(
                crate::terrain::populate_world_system::PopulateWorldSystem::<
                    TestEnvironment,
                    crate::terrain::DefaultDemTransferables,
                >::new(kernel),
            ),
        }
    }
}

pub(super) struct Fixture {
    pub source: Source,
    pub kernel: Rc<Kernel<TestEnvironment>>,
    pub context: MapContext,
    pub request: Box<dyn System>,
    pub populate: Box<dyn System>,
    kind: Kind,
}
impl Fixture {
    pub async fn new(kind: Kind, multiple: bool) -> Self {
        let source = Source::new().await;
        let (_, renderer) = create_headless_renderer(16, 16, None)
            .await
            .expect("renderer");
        let size = PhysicalSize::new(16, 16).expect("size");
        let kernel = Rc::new(
            KernelBuilder::new()
                .with_map_window_config(HeadlessMapWindowConfig::new(size))
                .with_http_client(ReqwestHttpClient::new::<String>(None))
                .with_scheduler(NopScheduler)
                .with_apc(Calls::default())
                .build()
                .expect("kernel"),
        );
        let mut world = World::default();
        world.resources.insert(FrameInput::default());
        let context = MapContext {
            renderer,
            style: kind.style(&source.url, multiple),
            world,
            view_state: ViewState::new(
                size,
                WorldCoords::from((256.0, 256.0)),
                Zoom::new(0.0),
                cgmath::Deg(0.0),
                cgmath::Rad(0.64),
            ),
        };
        Self {
            request: kind.request(&kernel),
            populate: kind.populate(&kernel),
            source,
            kernel,
            context,
            kind,
        }
    }
    pub fn frame(&mut self, millis: u64) {
        self.context
            .world
            .resources
            .get_or_init_mut::<FrameInput>()
            .timestamp = Duration::from_millis(millis);
        self.request.run(&mut self.context).expect("request frame");
    }
    pub async fn receive(&mut self) {
        self.kernel.apc().complete().await;
        self.populate
            .run(&mut self.context)
            .expect("populate frame");
    }
    pub fn loaded(&self) -> bool {
        match self.kind {
            Kind::Raster => self
                .context
                .world
                .tiles
                .query::<&RasterLayersDataComponent>(Default::default())
                .is_some_and(RasterLayersDataComponent::has_image),
            Kind::Dem => matches!(
                self.context
                    .world
                    .tiles
                    .query::<&DemTileComponent>(Default::default()),
                Some(DemTileComponent::Loaded(_))
            ),
        }
    }
}
