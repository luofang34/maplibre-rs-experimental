use maplibre::{
    environment::{OffscreenKernel, OffscreenKernelConfig},
    io::source_client::{HttpSourceClient, SourceClient},
};
use maplibre_winit::WinitEnvironment;

use crate::platform::{
    self,
    http_client::{web_http_client, WebHttpClient},
    UsedOffscreenKernelEnvironment,
};

/// Offscreen workers fetch map sources with the browser HTTP API.
pub struct WHATWGOffscreenKernelEnvironment(OffscreenKernelConfig);

impl OffscreenKernel for WHATWGOffscreenKernelEnvironment {
    type HttpClient = WebHttpClient;

    fn create(config: OffscreenKernelConfig) -> Self {
        WHATWGOffscreenKernelEnvironment(config)
    }

    fn source_client(&self) -> SourceClient<Self::HttpClient> {
        SourceClient::new(HttpSourceClient::new(web_http_client()))
            .with_asset_cache(self.0.asset_cache.clone())
    }
}

#[cfg(not(target_feature = "atomics"))]
pub(crate) type CurrentEnvironment = WinitEnvironment<
    maplibre::io::scheduler::NopScheduler,
    WebHttpClient,
    UsedOffscreenKernelEnvironment,
    platform::singlethreaded::apc::PassingAsyncProcedureCall,
    (),
>;

#[cfg(target_feature = "atomics")]
pub(crate) type CurrentEnvironment = WinitEnvironment<
    platform::multithreaded::pool_scheduler::WebWorkerPoolScheduler,
    WebHttpClient,
    UsedOffscreenKernelEnvironment,
    maplibre::io::apc::SchedulerAsyncProcedureCall<
        UsedOffscreenKernelEnvironment,
        platform::multithreaded::pool_scheduler::WebWorkerPoolScheduler,
    >,
    (),
>;
