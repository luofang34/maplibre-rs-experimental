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
    io::apc::{
        AsyncProcedure, AsyncProcedureCall, CallError, Context, Input, IntoMessage, Message,
        SendError,
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

/// What the workers sent back, and a signal for each message sent.
#[derive(Clone, Default)]
pub(super) struct Replies(Arc<Mutex<Vec<Message>>>, Arc<tokio::sync::Notify>);
impl Context for Replies {
    fn send_back<T: IntoMessage>(&self, message: T) -> Result<(), SendError> {
        self.0.lock().expect("replies").push(message.into());
        self.1.notify_one();
        Ok(())
    }
}

pub(super) type Procedure = AsyncProcedure<ReqwestOffscreenKernelEnvironment, Replies>;
#[derive(Default)]
pub(super) struct Calls {
    pending: RefCell<Vec<(Input, Procedure)>>,
    replies: Replies,
    /// Cloned for every call, so the calls share one asset cache as a map's workers do.
    config: OffscreenKernelConfig,
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
    /// Waits until a worker sends something back, or returns at once if one has since the
    /// last wait.
    pub async fn replied(&self) {
        self.replies.1.notified().await;
    }
    pub async fn complete(&self) {
        self.run_requests(self.take_requests()).await;
    }
    pub fn take_requests(&self) -> Vec<(Input, Procedure)> {
        std::mem::take(&mut *self.pending.borrow_mut())
    }
    pub async fn run_requests(&self, work: Vec<(Input, Procedure)>) {
        for (input, procedure) in work {
            procedure(
                input,
                self.replies.clone(),
                ReqwestOffscreenKernelEnvironment::create(self.config.clone()),
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
    type HttpClient = ReqwestHttpClient;
    type OffscreenKernelEnvironment = ReqwestOffscreenKernelEnvironment;
}

#[derive(Clone, Copy)]
pub(super) enum Kind {
    Raster,
    Dem,
    Vector,
}
impl Kind {
    fn style(self, url: &str, multiple: bool) -> Style {
        if matches!(self, Self::Vector) {
            return vector_style(url, multiple);
        }
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
                crate::terrain::DefaultDemTransferables,
            >::new(kernel)),
        }
    }
    fn populate(self, kernel: &Rc<Kernel<TestEnvironment>>) -> Box<dyn System> {
        match self {
            Self::Vector => Box::new(crate::vector::populate_world_system::PopulateWorldSystem::<
                TestEnvironment,
                crate::vector::DefaultVectorTransferables,
            >::new(kernel)),
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
    /// Direct vector requests made so far; each after the first refreshes the tile.
    direct_requests: u32,
}
impl Fixture {
    pub async fn new(kind: Kind, multiple: bool) -> Self {
        let source = Source::new().await;
        let http_client =
            ReqwestHttpClient::with_in_process_routes::<String>(None, source.routes());
        let calls = Calls {
            config: OffscreenKernelConfig {
                loader: Some(crate::io::resource_loader::SharedLoader::new(
                    http_client.clone(),
                )),
                ..OffscreenKernelConfig::default()
            },
            ..Calls::default()
        };
        let (_, renderer) = create_headless_renderer(16, 16, None)
            .await
            .expect("renderer");
        let size = PhysicalSize::new(16, 16).expect("size");
        let kernel = Rc::new(
            KernelBuilder::new()
                .with_map_window_config(HeadlessMapWindowConfig::new(size))
                .with_http_client(http_client)
                .with_apc(calls)
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
            direct_requests: 0,
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
    /// Admits a vector request for the origin tile outside the request system, as it would.
    pub fn direct_vector_input(&mut self) -> Input {
        use crate::io::tile_retry::{next_attempt, started, RequestKind};
        let world = &mut self.context.world;
        let attempt = next_attempt(world);
        started(
            world,
            Default::default(),
            RequestKind::Vector,
            attempt,
            None,
        );
        let mut input = Input::new(Default::default(), self.context.style.clone(), attempt);
        // A tile requested again stands for its data being due anew, as the request system
        // marks a refresh or a retry.
        input.refetch = self.direct_requests > 0;
        self.direct_requests = self.direct_requests.wrapping_add(1);
        input
    }
    pub async fn receive(&mut self) {
        self.kernel.apc().complete().await;
        self.populate
            .run(&mut self.context)
            .expect("populate frame");
    }
    pub fn loaded(&self) -> bool {
        match self.kind {
            Kind::Vector => self.context.world.tiles
                .query::<&crate::vector::VectorLayerBucketComponent>(Default::default())
                .is_some_and(|component| component.done && !component.failed && component.layers.iter().any(|layer| matches!(layer, crate::vector::VectorLayerBucket::AvailableLayer(bucket) if !bucket.buffer.buffer.indices.is_empty()))),
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

fn vector_style(url: &str, multiple: bool) -> Style {
    let mut json = serde_json::json!({"version":8,
        "sources":{"source":{"type":"vector","tiles":[format!("{url}/unstable/{{z}}/{{x}}/{{y}}")],"maxzoom":0}},
        "layers":[{"id":"land","source":"source","source-layer":"land","type":"fill","paint":{"fill-color":"#00ff00"}}]});
    if multiple {
        json["sources"]["healthy"] = serde_json::json!({"type":"vector","tiles":[format!("{url}/healthy/{{z}}/{{x}}/{{y}}")],"maxzoom":0});
        json["layers"].as_array_mut().expect("layers").insert(0, serde_json::json!({"id":"healthy","source":"healthy","source-layer":"land","type":"fill","paint":{"fill-color":"#ff0000"}}));
    }
    serde_json::from_value(json).expect("vector style")
}
