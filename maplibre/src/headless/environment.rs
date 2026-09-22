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
pub struct HeadlessEnvironment;

#[cfg(not(target_arch = "wasm32"))]
impl Environment for HeadlessEnvironment {
    type MapWindowConfig = HeadlessMapWindowConfig;
    type AsyncProcedureCall =
        SchedulerAsyncProcedureCall<Self::OffscreenKernelEnvironment, Self::Scheduler>;
    type Scheduler = TokioScheduler;
    type HttpClient = ReqwestHttpClient;
    type OffscreenKernelEnvironment = ReqwestOffscreenKernelEnvironment;
}

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub(super) use web::create_kernel;
#[cfg(target_arch = "wasm32")]
pub use web::HeadlessEnvironment;

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn create_kernel(
    size: crate::window::PhysicalSize,
    cache_path: Option<String>,
) -> crate::kernel::Kernel<HeadlessEnvironment> {
    let client = ReqwestHttpClient::new(cache_path.clone());
    crate::kernel::KernelBuilder::new()
        .with_map_window_config(HeadlessMapWindowConfig::new(size))
        .with_http_client(client)
        .with_apc(SchedulerAsyncProcedureCall::new(
            TokioScheduler::new(),
            crate::environment::OffscreenKernelConfig {
                cache_directory: cache_path,
            },
        ))
        .with_scheduler(TokioScheduler::new())
        .build()
}
