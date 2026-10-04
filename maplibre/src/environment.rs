//! Platform service types and serializable worker configuration.

#![deny(missing_docs)]

use serde::{Deserialize, Serialize};

use crate::{
    io::{
        apc::AsyncProcedureCall,
        scheduler::Scheduler,
        source_client::{HttpClient, SourceClient},
    },
    sdf::assets::{AssetCache, ImageProviders},
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
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct OffscreenKernelConfig {
    /// Filesystem cache directory for hosts that support persistent HTTP caching.
    /// `None` disables that cache; browser-only kernels may ignore the setting.
    pub cache_directory: Option<String>,
    /// Decoded glyphs and sprites shared by every call made from clones of this configuration.
    /// It is not serialized: a worker that receives the configuration over a channel starts
    /// with its own empty cache.
    #[serde(skip)]
    pub asset_cache: AssetCache,
    /// The providers of images labels name and no sprite supplies, shared like the asset
    /// cache. A worker that receives its configuration over a channel registers its own.
    #[serde(skip)]
    pub image_providers: ImageProviders,
    /// The loader the map thread uses, so in-process workers fetch through the same one.
    /// Like the asset cache it is not serialized: a worker in another process or a browser
    /// worker builds its platform's own loader.
    #[serde(skip)]
    pub loader: Option<crate::io::resource_loader::SharedLoader>,
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
