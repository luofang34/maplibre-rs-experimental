//! Platform service types and serializable worker configuration.

#![deny(missing_docs)]

use serde::{Deserialize, Serialize};

use crate::{
    io::{
        apc::AsyncProcedureCall,
        scheduler::Scheduler,
        source_client::{HttpClient, SourceClient},
    },
    window::MapWindowConfig,
};

/// Platform services selected together at compile time.
/// The worker transport must support the same offscreen kernel used to fetch source data.
pub trait Environment: 'static {
    /// Factory for the map's host window.
    type MapWindowConfig: MapWindowConfig;

    /// Request/reply transport that runs procedures with the offscreen kernel.
    type AsyncProcedureCall: AsyncProcedureCall<Self::OffscreenKernelEnvironment>;

    /// Executor available to systems running on the map thread.
    type Scheduler: Scheduler;

    /// HTTP implementation used by this environment's source loader.
    type HttpClient: HttpClient;

    /// Services that a worker can create from serialized configuration.
    type OffscreenKernelEnvironment: OffscreenKernel;
}

/// Configuration passed across worker boundaries to construct source services.
#[derive(Serialize, Deserialize, Clone)]
pub struct OffscreenKernelConfig {
    /// Filesystem cache directory for hosts that support persistent HTTP caching.
    /// `None` disables that cache; browser-only kernels may ignore the setting.
    pub cache_directory: Option<String>,
}

/// Worker-side services, independent of window and GPU ownership.
pub trait OffscreenKernel: Send + Sync + 'static {
    /// HTTP implementation used by this environment's source loader.
    type HttpClient: HttpClient;
    /// Constructs worker services from the configuration supplied at worker startup.
    fn create(config: OffscreenKernelConfig) -> Self;

    /// Creates a source loader using this worker's HTTP and cache configuration.
    fn source_client(&self) -> SourceClient<Self::HttpClient>;
}
