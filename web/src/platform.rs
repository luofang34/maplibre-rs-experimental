use std::sync::OnceLock;

use http_client::{web_http_client, WebHttpClient};
pub use maplibre::platform::http_client;
use maplibre::{
    environment::{OffscreenKernel, OffscreenKernelConfig},
    io::source_client::{HttpSourceClient, SourceClient},
    sdf::assets::ImageProviders,
};

/// Offscreen workers fetch map sources with the browser HTTP API.
pub struct WHATWGOffscreenKernelEnvironment(OffscreenKernelConfig);

impl OffscreenKernel for WHATWGOffscreenKernelEnvironment {
    type HttpClient = WebHttpClient;

    fn create(config: OffscreenKernelConfig) -> Self {
        WHATWGOffscreenKernelEnvironment(config)
    }

    fn source_client(&self) -> SourceClient<Self::HttpClient> {
        SourceClient::new(HttpSourceClient::new(web_http_client()))
            .with_asset_cache(self.0.asset_cache.clone())
            .with_image_providers(self.0.image_providers.clone())
    }
}

/// The offscreen worker kernel environment selected for browser builds.
pub type UsedOffscreenKernelEnvironment = WHATWGOffscreenKernelEnvironment;

/// The providers of images that labels name and no sprite supplies, for every map and worker
/// of this WebAssembly module. Workers of a single-threaded build run their own copy of the
/// module, so a host registers its providers in each worker as well as on the page.
static IMAGE_PROVIDERS: OnceLock<ImageProviders> = OnceLock::new();

/// The registry this module's tile workers ask for provided images.
pub fn image_providers() -> &'static ImageProviders {
    IMAGE_PROVIDERS.get_or_init(ImageProviders::default)
}

#[cfg(any(test, target_arch = "wasm32"))]
pub(crate) mod alloc;

#[cfg(target_feature = "atomics")]
pub mod multithreaded;

#[cfg(not(target_feature = "atomics"))]
pub mod singlethreaded;

#[cfg(target_feature = "atomics")]
pub type UsedRasterTransferables = maplibre::raster::DefaultRasterTransferables;
#[cfg(not(target_feature = "atomics"))]
pub type UsedRasterTransferables = singlethreaded::transferables::FlatTransferables;

#[cfg(target_feature = "atomics")]
pub type UsedDemTransferables = maplibre::terrain::DefaultDemTransferables;
#[cfg(not(target_feature = "atomics"))]
pub type UsedDemTransferables = singlethreaded::transferables::FlatTransferables;

#[cfg(target_feature = "atomics")]
pub type UsedVectorTransferables = maplibre::vector::DefaultVectorTransferables;
#[cfg(not(target_feature = "atomics"))]
pub type UsedVectorTransferables = singlethreaded::transferables::FlatTransferables;
