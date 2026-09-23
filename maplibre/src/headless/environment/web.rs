//! Supplied browser tiles have no network or native runtime dependency.
use crate::{
    environment::{Environment, OffscreenKernel, OffscreenKernelConfig},
    headless::window::HeadlessMapWindowConfig,
    io::{
        apc::SchedulerAsyncProcedureCall,
        scheduler::NopScheduler,
        source_client::{HttpClient, HttpSourceClient, SourceClient, SourceFetchError},
    },
    kernel::{Kernel, KernelBuilder},
    window::PhysicalSize,
};

/// Browser environment for decoded tiles supplied by the host.
pub struct HeadlessEnvironment;
/// Rejects network requests because this environment has no source loader.
#[derive(Clone)]
pub struct SuppliedTileClient;
/// Offscreen kernel with no native runtime or I/O dependencies.
pub struct SuppliedTileKernel;

#[async_trait::async_trait(?Send)]
impl HttpClient for SuppliedTileClient {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        Err(SourceFetchError::not_found(url))
    }
}
impl OffscreenKernel for SuppliedTileKernel {
    type HttpClient = SuppliedTileClient;
    fn create(_config: OffscreenKernelConfig) -> Self {
        Self
    }
    fn source_client(&self) -> SourceClient<Self::HttpClient> {
        SourceClient::new(HttpSourceClient::new(SuppliedTileClient))
    }
}
impl Environment for HeadlessEnvironment {
    type MapWindowConfig = HeadlessMapWindowConfig;
    type AsyncProcedureCall = SchedulerAsyncProcedureCall<SuppliedTileKernel, NopScheduler>;
    type Scheduler = NopScheduler;
    type HttpClient = SuppliedTileClient;
    type OffscreenKernelEnvironment = SuppliedTileKernel;
}
pub(crate) fn create_kernel(
    size: PhysicalSize,
    _cache_path: Option<String>,
) -> Kernel<HeadlessEnvironment> {
    KernelBuilder::new()
        .with_map_window_config(HeadlessMapWindowConfig::new(size))
        .with_http_client(SuppliedTileClient)
        .with_apc(SchedulerAsyncProcedureCall::new(
            NopScheduler,
            OffscreenKernelConfig {
                cache_directory: None,
            },
        ))
        .with_scheduler(NopScheduler)
        .build()
}
