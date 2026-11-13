//! Platform services for maps rendered without a presentation window.

#![deny(missing_docs)]

#[cfg(all(not(target_arch = "wasm32"), feature = "native-io"))]
use crate::{
    environment::Environment,
    headless::window::HeadlessMapWindowConfig,
    io::apc::SchedulerAsyncProcedureCall,
    platform::{
        http_client::ReqwestHttpClient, scheduler::TokioScheduler,
        ReqwestOffscreenKernelEnvironment,
    },
};

#[cfg(all(not(target_arch = "wasm32"), feature = "native-io"))]
/// Native offscreen services using the Tokio scheduler and Reqwest source loader.
pub struct HeadlessEnvironment;

#[cfg(all(not(target_arch = "wasm32"), feature = "native-io"))]
impl Environment for HeadlessEnvironment {
    type MapWindowConfig = HeadlessMapWindowConfig;
    type AsyncProcedureCall =
        SchedulerAsyncProcedureCall<Self::OffscreenKernelEnvironment, TokioScheduler>;
    type HttpClient = crate::io::resource_loader::SharedLoader;
    type OffscreenKernelEnvironment = ReqwestOffscreenKernelEnvironment;
}

#[cfg(all(not(target_arch = "wasm32"), not(feature = "native-io")))]
/// Runtime-agnostic offscreen environment when `native-io` is disabled.
pub struct HeadlessEnvironment;

#[cfg(all(not(target_arch = "wasm32"), not(feature = "native-io")))]
impl crate::environment::Environment for HeadlessEnvironment {
    type MapWindowConfig = crate::headless::window::HeadlessMapWindowConfig;
    type AsyncProcedureCall = crate::io::apc::SchedulerAsyncProcedureCall<
        LoaderKernel,
        crate::io::scheduler::NopScheduler,
    >;
    type HttpClient = crate::io::resource_loader::SharedLoader;
    type OffscreenKernelEnvironment = LoaderKernel;
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

#[cfg(all(not(target_arch = "wasm32"), feature = "native-io"))]
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

#[cfg(all(not(target_arch = "wasm32"), not(feature = "native-io")))]
pub(crate) fn create_kernel(
    size: crate::window::PhysicalSize,
    cache_path: Option<String>,
    loader: Option<crate::io::resource_loader::SharedLoader>,
    asset_cache: Option<crate::sdf::assets::AssetCache>,
) -> Result<crate::kernel::Kernel<HeadlessEnvironment>, crate::kernel::KernelBuildError> {
    let loader = loader.unwrap_or_else(|| loader_kernel::platform_loader(cache_path.clone()));
    let asset_cache = asset_cache.unwrap_or_default();
    crate::kernel::KernelBuilder::new()
        .with_map_window_config(crate::headless::window::HeadlessMapWindowConfig::new(size))
        .with_http_client(loader.clone())
        .with_asset_cache(asset_cache.clone())
        .with_apc(crate::io::apc::SchedulerAsyncProcedureCall::new(
            crate::io::scheduler::NopScheduler,
            crate::environment::OffscreenKernelConfig {
                cache_directory: cache_path,
                asset_cache,
                loader: Some(loader),
                ..Default::default()
            },
        ))
        .build()
}
