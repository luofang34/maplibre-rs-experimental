use crate::{
    environment::{Environment, OffscreenKernel, OffscreenKernelConfig},
    io::{
        apc::SchedulerAsyncProcedureCall,
        scheduler::NopScheduler,
        source_client::{HttpClient, HttpSourceClient, SourceClient, SourceFetchError},
    },
    kernel::KernelBuilder,
    window::{MapWindow, MapWindowConfig, PhysicalSize, WindowCreateError},
};

pub(super) struct TestEnvironment;
pub(super) struct WorkerKernel;
#[derive(Clone)]
pub(super) struct WindowConfig(pub u32);
pub(super) struct Window(PhysicalSize);
#[derive(Clone)]
pub(super) struct Client(pub &'static str);

impl MapWindow for Window {
    fn size(&self) -> PhysicalSize {
        self.0
    }
}
impl MapWindowConfig for WindowConfig {
    type MapWindow = Window;
    fn create(&self) -> Result<Window, WindowCreateError> {
        Ok(Window(PhysicalSize::new(self.0, 1).expect("fixture width")))
    }
}
#[cfg_attr(not(feature = "thread-safe-futures"), async_trait::async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait::async_trait)]
impl HttpClient for Client {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        Ok(format!("{}:{url}", self.0).into_bytes())
    }
}
impl OffscreenKernel for WorkerKernel {
    type HttpClient = Client;
    fn create(_: OffscreenKernelConfig) -> Self {
        Self
    }
    fn source_client(&self) -> SourceClient<Client> {
        SourceClient::new(HttpSourceClient::new(Client("worker")))
    }
}
impl Environment for TestEnvironment {
    type MapWindowConfig = WindowConfig;
    type AsyncProcedureCall = SchedulerAsyncProcedureCall<WorkerKernel, NopScheduler>;
    type Scheduler = NopScheduler;
    type HttpClient = Client;
    type OffscreenKernelEnvironment = WorkerKernel;
}

pub(super) fn builder_without(missing: &str) -> KernelBuilder<TestEnvironment> {
    let mut builder = KernelBuilder::new();
    if missing != "window" {
        builder = builder.with_map_window_config(WindowConfig(42));
    }
    if missing != "apc" {
        builder = builder.with_apc(SchedulerAsyncProcedureCall::new(
            NopScheduler,
            OffscreenKernelConfig {
                cache_directory: None,
            },
        ));
    }
    if missing != "scheduler" {
        builder = builder.with_scheduler(NopScheduler);
    }
    if missing != "http" {
        builder = builder.with_http_client(Client("source"));
    }
    builder
}
