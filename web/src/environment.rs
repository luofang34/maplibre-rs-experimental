#[cfg(feature = "winit-window")]
use maplibre_winit::WinitEnvironment;

pub use crate::platform::WHATWGOffscreenKernelEnvironment;
#[cfg(feature = "winit-window")]
use crate::platform::{self, http_client::WebHttpClient, UsedOffscreenKernelEnvironment};

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
