use maplibre::{
    environment::{OffscreenKernel, OffscreenKernelConfig},
    io::source_client::{HttpSourceClient, SourceClient},
};
use maplibre_winit::WinitEnvironment;

use crate::platform::{self, http_client::WHATWGFetchHttpClient, UsedOffscreenKernelEnvironment};

/// Offscreen workers fetch map sources with the browser HTTP API.
pub struct WHATWGOffscreenKernelEnvironment;

impl OffscreenKernel for WHATWGOffscreenKernelEnvironment {
    type HttpClient = WHATWGFetchHttpClient;

    fn create(_config: OffscreenKernelConfig) -> Self {
        WHATWGOffscreenKernelEnvironment
    }

    fn source_client(&self) -> SourceClient<Self::HttpClient> {
        SourceClient::new(HttpSourceClient::new(WHATWGFetchHttpClient::default()))
    }
}

#[cfg(not(target_feature = "atomics"))]
pub(crate) type CurrentEnvironment = WinitEnvironment<
    maplibre::io::scheduler::NopScheduler,
    WHATWGFetchHttpClient,
    UsedOffscreenKernelEnvironment,
    platform::singlethreaded::apc::PassingAsyncProcedureCall,
    (),
>;

#[cfg(target_feature = "atomics")]
pub(crate) type CurrentEnvironment = WinitEnvironment<
    platform::multithreaded::pool_scheduler::WebWorkerPoolScheduler,
    WHATWGFetchHttpClient,
    UsedOffscreenKernelEnvironment,
    maplibre::io::apc::SchedulerAsyncProcedureCall<
        UsedOffscreenKernelEnvironment,
        platform::multithreaded::pool_scheduler::WebWorkerPoolScheduler,
    >,
    (),
>;
