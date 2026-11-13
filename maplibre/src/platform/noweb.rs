//! Native HTTP, task scheduling and Tokio runtime entry points.

#![deny(missing_docs)]

use std::{
    future::Future,
    sync::atomic::{AtomicUsize, Ordering},
};

use crate::{
    environment::{OffscreenKernel, OffscreenKernelConfig},
    io::{
        resource_loader::SharedLoader,
        source_client::{HttpSourceClient, SourceClient},
    },
    platform::http_client::ReqwestHttpClient,
};

pub mod http_client;
pub mod scheduler;
pub mod trace;

/// Creates an I/O- and timer-enabled Tokio runtime and blocks until `future` completes.
/// The runtime is dropped before this function returns; detached work must finish before
/// the supplied future if its results are required.
///
/// # Panics
/// Panics if runtime creation fails or if called from within an asynchronous runtime.
#[allow(clippy::expect_used)]
pub fn run_multithreaded<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_multi_thread()
        .enable_io()
        .enable_time()
        .thread_name_fn(|| {
            static ATOMIC_ID: AtomicUsize = AtomicUsize::new(0);
            let id = ATOMIC_ID.fetch_add(1, Ordering::SeqCst);
            format!("maplibre-rs-pool-{id}")
        })
        .on_thread_start(|| {
            #[cfg(feature = "trace")]
            tracing_tracy::client::set_thread_name!("tokio-runtime-worker");

            log::info!("Worker thread started")
        })
        .build()
        .expect("failed to build Tokio runtime")
        .block_on(future)
}

/// Drives `future` to completion on a current-thread Tokio runtime with I/O and timers enabled.
#[cfg(test)]
pub(crate) fn run_current_thread<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build current-thread Tokio runtime")
        .block_on(future)
}

/// Runs CPU-bound `work` on the current Tokio runtime's blocking pool when inside a runtime,
/// or inline when called outside one.
pub(crate) async fn run_cpu_bound<R: Send + 'static>(
    work: impl FnOnce() -> R + Send + 'static,
) -> Result<R, Box<dyn std::error::Error + Send + Sync>> {
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        return runtime
            .spawn_blocking(work)
            .await
            .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>);
    }
    Ok(work())
}

/// Worker services that construct a Reqwest source client using the configured cache directory.
pub struct ReqwestOffscreenKernelEnvironment(OffscreenKernelConfig);

impl OffscreenKernel for ReqwestOffscreenKernelEnvironment {
    type HttpClient = SharedLoader;

    fn create(config: OffscreenKernelConfig) -> Self {
        ReqwestOffscreenKernelEnvironment(config)
    }

    fn source_client(&self) -> SourceClient<Self::HttpClient> {
        let loader = self.0.loader.clone().unwrap_or_else(|| {
            SharedLoader::new(ReqwestHttpClient::new::<String>(
                self.0.cache_directory.clone(),
            ))
        });
        SourceClient::new(HttpSourceClient::new(loader))
            .with_asset_cache(self.0.asset_cache.clone())
            .with_image_providers(self.0.image_providers.clone())
    }
}
