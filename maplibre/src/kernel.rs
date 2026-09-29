//! Host services shared by the map and its plugins.

#![deny(missing_docs, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::{
    environment::Environment,
    io::source_client::{HttpSourceClient, SourceClient},
};

/// Owns the window factory, executor, worker transport and source loader selected by [`Environment`].
/// Maps share the kernel with their plugins for the lifetime of those services.
pub struct Kernel<E: Environment> {
    map_window_config: E::MapWindowConfig,
    apc: E::AsyncProcedureCall,
    scheduler: E::Scheduler,
    source_client: SourceClient<E::HttpClient>,
}

impl<E: Environment> Kernel<E> {
    /// Window factory used when a map is created.
    pub fn map_window_config(&self) -> &E::MapWindowConfig {
        &self.map_window_config
    }

    /// Worker request and reply transport for this map.
    pub fn apc(&self) -> &E::AsyncProcedureCall {
        &self.apc
    }

    /// Executor for asynchronous tasks submitted by map systems.
    pub fn scheduler(&self) -> &E::Scheduler {
        &self.scheduler
    }

    /// Source loader used by the map's calling thread.
    pub fn source_client(&self) -> &SourceClient<E::HttpClient> {
        &self.source_client
    }
}

/// A required host service was not supplied to [`KernelBuilder`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum KernelBuildError {
    /// No window configuration was supplied.
    #[error("kernel requires a map window configuration")]
    MissingWindowConfig,
    /// No worker request and reply transport was supplied.
    #[error("kernel requires an asynchronous procedure call transport")]
    MissingAsyncProcedureCall,
    /// No asynchronous executor was supplied.
    #[error("kernel requires a scheduler")]
    MissingScheduler,
    /// No HTTP implementation was supplied.
    #[error("kernel requires an HTTP client")]
    MissingHttpClient,
}

/// Collects the four required host services before constructing a [`Kernel`].
pub struct KernelBuilder<E: Environment> {
    map_window_config: Option<E::MapWindowConfig>,
    apc: Option<E::AsyncProcedureCall>,
    scheduler: Option<E::Scheduler>,
    http_client: Option<E::HttpClient>,
}

impl<E: Environment> Default for KernelBuilder<E> {
    fn default() -> Self {
        Self::new()
    }
}

impl<E: Environment> KernelBuilder<E> {
    /// Starts with no services; all four `with_*` methods are required before [`Self::build`].
    pub fn new() -> Self {
        Self {
            scheduler: None,
            apc: None,
            http_client: None,
            map_window_config: None,
        }
    }

    /// Sets the window factory, replacing any configuration already supplied.
    pub fn with_map_window_config(mut self, map_window_config: E::MapWindowConfig) -> Self {
        self.map_window_config = Some(map_window_config);
        self
    }

    /// Sets the executor, replacing any scheduler already supplied.
    pub fn with_scheduler(mut self, scheduler: E::Scheduler) -> Self {
        self.scheduler = Some(scheduler);
        self
    }

    /// Sets the worker transport, replacing any transport already supplied.
    pub fn with_apc(mut self, apc: E::AsyncProcedureCall) -> Self {
        self.apc = Some(apc);
        self
    }

    /// Sets the source HTTP implementation, replacing any client already supplied.
    pub fn with_http_client(mut self, http_client: E::HttpClient) -> Self {
        self.http_client = Some(http_client);
        self
    }

    /// Consumes the services without creating a window or submitting work.
    /// Returns the first missing dependency in window, transport, scheduler, HTTP order.
    pub fn build(self) -> Result<Kernel<E>, KernelBuildError> {
        Ok(Kernel {
            map_window_config: self
                .map_window_config
                .ok_or(KernelBuildError::MissingWindowConfig)?,
            apc: self
                .apc
                .ok_or(KernelBuildError::MissingAsyncProcedureCall)?,
            scheduler: self.scheduler.ok_or(KernelBuildError::MissingScheduler)?,
            source_client: SourceClient::new(HttpSourceClient::new(
                self.http_client
                    .ok_or(KernelBuildError::MissingHttpClient)?,
            )),
        })
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
