use std::sync::OnceLock;

use maplibre::sdf::assets::ImageProviders;

use crate::WHATWGOffscreenKernelEnvironment;

pub use maplibre::platform::http_client;

/// The providers of images that labels name and no sprite supplies, for every map and worker
/// of this WebAssembly module. Workers of a single-threaded build run their own copy of the
/// module, so a host registers its providers in each worker as well as on the page.
static IMAGE_PROVIDERS: OnceLock<ImageProviders> = OnceLock::new();

/// The registry this module's tile workers ask for provided images.
pub fn image_providers() -> &'static ImageProviders {
    IMAGE_PROVIDERS.get_or_init(ImageProviders::default)
}

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

pub type UsedOffscreenKernelEnvironment = WHATWGOffscreenKernelEnvironment;
