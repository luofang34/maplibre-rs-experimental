use maplibre::{
    environment::{OffscreenKernel, OffscreenKernelConfig},
    io::source_client::{HttpSourceClient, SourceClient},
};
#[cfg(feature = "winit-window")]
use maplibre_winit::WinitEnvironment;

use crate::platform::http_client::{web_http_client, WebHttpClient};
#[cfg(feature = "winit-window")]
use crate::platform::{self, UsedOffscreenKernelEnvironment};

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
            .with_image_providers(self.0.image_providers.clone())
    }
}

/// The environment of a browser map, which a host's plugins are written for: workers reached
/// by message passing in a single-threaded build, or a shared-memory pool with atomics.
#[cfg(all(feature = "winit-window", not(target_feature = "atomics")))]
pub type CurrentEnvironment = WinitEnvironment<
    WebHttpClient,
    UsedOffscreenKernelEnvironment,
    platform::singlethreaded::apc::PassingAsyncProcedureCall,
    (),
>;

/// The environment of a browser map, which a host's plugins are written for: workers reached
/// by message passing in a single-threaded build, or a shared-memory pool with atomics.
#[cfg(all(feature = "winit-window", target_feature = "atomics"))]
pub type CurrentEnvironment = WinitEnvironment<
    WebHttpClient,
    UsedOffscreenKernelEnvironment,
    maplibre::io::apc::SchedulerAsyncProcedureCall<
        UsedOffscreenKernelEnvironment,
        platform::multithreaded::pool_scheduler::WebWorkerPoolScheduler,
    >,
    (),
>;
