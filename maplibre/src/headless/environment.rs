//! Platform services for maps rendered without a presentation window.

#![deny(missing_docs)]

#[cfg(not(target_arch = "wasm32"))]
use crate::{
    environment::Environment,
    headless::window::HeadlessMapWindowConfig,
    io::apc::SchedulerAsyncProcedureCall,
    platform::{
        http_client::ReqwestHttpClient, scheduler::TokioScheduler,
        ReqwestOffscreenKernelEnvironment,
    },
};

#[cfg(not(target_arch = "wasm32"))]
/// Native offscreen services using the Tokio scheduler and Reqwest source loader.
pub struct HeadlessEnvironment;

#[cfg(not(target_arch = "wasm32"))]
impl Environment for HeadlessEnvironment {
    type MapWindowConfig = HeadlessMapWindowConfig;
    type AsyncProcedureCall =
        SchedulerAsyncProcedureCall<Self::OffscreenKernelEnvironment, TokioScheduler>;
    type HttpClient = crate::io::resource_loader::SharedLoader;
    type OffscreenKernelEnvironment = ReqwestOffscreenKernelEnvironment;
}

mod loader_kernel;
mod supplied;
#[cfg(target_arch = "wasm32")]
mod web;

pub use loader_kernel::LoaderKernel;
pub use supplied::SuppliedTileClient;
#[cfg(target_arch = "wasm32")]
pub(super) use web::create_kernel;
#[cfg(target_arch = "wasm32")]
pub use web::HeadlessEnvironment;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn create_kernel(
    size: crate::window::PhysicalSize,
    cache_path: Option<String>,
    loader: Option<crate::io::resource_loader::SharedLoader>,
    asset_cache: Option<crate::sdf::assets::AssetCache>,
) -> Result<crate::kernel::Kernel<HeadlessEnvironment>, crate::kernel::KernelBuildError> {
    let loader = loader.unwrap_or_else(|| {
        crate::io::resource_loader::SharedLoader::new(ReqwestHttpClient::new(cache_path.clone()))
    });
    let asset_cache = asset_cache.unwrap_or_default();
    crate::kernel::KernelBuilder::new()
        .with_map_window_config(HeadlessMapWindowConfig::new(size))
        .with_http_client(loader.clone())
        .with_asset_cache(asset_cache.clone())
        .with_apc(SchedulerAsyncProcedureCall::new(
            TokioScheduler::new(),
            crate::environment::OffscreenKernelConfig {
                cache_directory: cache_path,
                asset_cache,
                loader: Some(loader),
                ..Default::default()
            },
        ))
        .build()
}
