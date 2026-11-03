//! Host services shared by the map and its plugins.

#![deny(missing_docs, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::{
    environment::Environment,
    io::source_client::{HttpSourceClient, SourceClient},
    sdf::assets::AssetCache,
};

/// Owns the window factory, worker transport and source loader selected by [`Environment`].
/// Maps share the kernel with their plugins for the lifetime of those services.
pub struct Kernel<E: Environment> {
    map_window_config: E::MapWindowConfig,
    apc: E::AsyncProcedureCall,
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
    /// No HTTP implementation was supplied.
    #[error("kernel requires an HTTP client")]
    MissingHttpClient,
}

/// Collects the three required host services before constructing a [`Kernel`].
pub struct KernelBuilder<E: Environment> {
    map_window_config: Option<E::MapWindowConfig>,
    apc: Option<E::AsyncProcedureCall>,
    http_client: Option<E::HttpClient>,
    asset_cache: Option<AssetCache>,
}

impl<E: Environment> Default for KernelBuilder<E> {
    fn default() -> Self {
        Self::new()
    }
}

impl<E: Environment> KernelBuilder<E> {
    /// Starts with no services; the window, transport and HTTP `with_*` methods are required before [`Self::build`].
    pub fn new() -> Self {
        Self {
            apc: None,
            http_client: None,
            map_window_config: None,
            asset_cache: None,
        }
    }

    /// Shares `asset_cache` with the map thread's [`SourceClient`].
    pub fn with_asset_cache(mut self, asset_cache: AssetCache) -> Self {
        self.asset_cache = Some(asset_cache);
        self
    }

    /// Sets the window factory, replacing any configuration already supplied.
    pub fn with_map_window_config(mut self, map_window_config: E::MapWindowConfig) -> Self {
        self.map_window_config = Some(map_window_config);
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
    /// Returns the first missing dependency in window, transport, HTTP order.
    pub fn build(self) -> Result<Kernel<E>, KernelBuildError> {
        let map_window_config = self
            .map_window_config
            .ok_or(KernelBuildError::MissingWindowConfig)?;
        let apc = self
            .apc
            .ok_or(KernelBuildError::MissingAsyncProcedureCall)?;
        let http_client = self
            .http_client
            .ok_or(KernelBuildError::MissingHttpClient)?;
        let mut source_client = SourceClient::new(HttpSourceClient::new(http_client));
        if let Some(asset_cache) = self.asset_cache {
            source_client = source_client.with_asset_cache(asset_cache);
        }
        Ok(Kernel {
            map_window_config,
            apc,
            source_client,
        })
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
