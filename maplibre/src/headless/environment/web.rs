//! Browser maps drawn on the host's GPU, fetching on the page's event loop.
use crate::{
    environment::{Environment, OffscreenKernelConfig},
    headless::{environment::LoaderKernel, window::HeadlessMapWindowConfig},
    io::{
        apc::SchedulerAsyncProcedureCall, resource_loader::SharedLoader, scheduler::LocalScheduler,
    },
    kernel::{Kernel, KernelBuilder},
    window::PhysicalSize,
};

/// Browser environment whose tile calls run as local futures on the page's thread.
pub struct HeadlessEnvironment;

impl Environment for HeadlessEnvironment {
    type MapWindowConfig = HeadlessMapWindowConfig;
    type AsyncProcedureCall = SchedulerAsyncProcedureCall<LoaderKernel, LocalScheduler>;
    type Scheduler = LocalScheduler;
    type HttpClient = SharedLoader;
    type OffscreenKernelEnvironment = LoaderKernel;
}

pub(crate) fn create_kernel(
    size: PhysicalSize,
    cache_path: Option<String>,
    loader: Option<SharedLoader>,
) -> Result<Kernel<HeadlessEnvironment>, crate::kernel::KernelBuildError> {
    // One loader for the map and every call, so in-flight requests and PMTiles directories are
    // shared instead of rebuilt per tile.
    let loader =
        loader.unwrap_or_else(|| super::loader_kernel::platform_loader(cache_path.clone()));
    KernelBuilder::new()
        .with_map_window_config(HeadlessMapWindowConfig::new(size))
        .with_http_client(loader.clone())
        .with_apc(SchedulerAsyncProcedureCall::new(
            LocalScheduler,
            OffscreenKernelConfig {
                cache_directory: cache_path,
                loader: Some(loader),
                ..Default::default()
            },
        ))
        .with_scheduler(LocalScheduler)
        .build()
}
