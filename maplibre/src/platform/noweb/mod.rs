//! Native HTTP, task scheduling and Tokio runtime entry points.

#![deny(missing_docs)]

use std::{
    future::Future,
    sync::atomic::{AtomicUsize, Ordering},
};

use crate::{
    environment::{OffscreenKernel, OffscreenKernelConfig},
    io::source_client::{HttpSourceClient, SourceClient},
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
        .unwrap()
        .block_on(future)
}

/// Worker services that construct a Reqwest source client using the configured cache directory.
pub struct ReqwestOffscreenKernelEnvironment(OffscreenKernelConfig);

impl OffscreenKernel for ReqwestOffscreenKernelEnvironment {
    type HttpClient = ReqwestHttpClient;

    fn create(config: OffscreenKernelConfig) -> Self {
        ReqwestOffscreenKernelEnvironment(config)
    }

    fn source_client(&self) -> SourceClient<Self::HttpClient> {
        SourceClient::new(HttpSourceClient::new(ReqwestHttpClient::new::<String>(
            self.0.cache_directory.clone(),
        )))
        .with_asset_cache(self.0.asset_cache.clone())
    }
}
