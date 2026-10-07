//! Tile workers that fetch through the loader the map thread was given.

use crate::{
    environment::{OffscreenKernel, OffscreenKernelConfig},
    io::{
        resource_loader::SharedLoader,
        source_client::{HttpSourceClient, SourceClient},
    },
};

/// Offscreen kernel fetching sources with the configuration's loader, or with the platform's
/// HTTP transport when the configuration carries none.
pub struct LoaderKernel(OffscreenKernelConfig);

impl OffscreenKernel for LoaderKernel {
    type HttpClient = SharedLoader;

    fn create(config: OffscreenKernelConfig) -> Self {
        Self(config)
    }

    fn source_client(&self) -> SourceClient<Self::HttpClient> {
        let loader = self
            .0
            .loader
            .clone()
            .unwrap_or_else(|| platform_loader(self.0.cache_directory.clone()));
        SourceClient::new(HttpSourceClient::new(loader))
            .with_asset_cache(self.0.asset_cache.clone())
            .with_image_providers(self.0.image_providers.clone())
    }
}

/// The browser's `fetch`, reading `pmtiles://` archives with range requests.
#[cfg(target_arch = "wasm32")]
pub(crate) fn platform_loader(_cache_directory: Option<String>) -> SharedLoader {
    SharedLoader::new(crate::platform::http_client::web_http_client())
}

/// Reqwest with the configured on-disk cache.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn platform_loader(cache_directory: Option<String>) -> SharedLoader {
    SharedLoader::new(crate::platform::http_client::ReqwestHttpClient::new(
        cache_directory,
    ))
}

#[cfg(test)]
mod tests;
