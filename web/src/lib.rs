//! Browser map initialization and worker transports.
#![deny(unused_imports)]

#[cfg(not(any(no_pendantic_os_check, target_arch = "wasm32")))]
compile_error!("web works only on wasm32.");

mod application;
mod environment;
mod error;
mod platform;
mod startup;

pub use application::{
    create_kernel, default_plugins, run_map, run_maplibre, MapPlugin, MapType, WebMapHost,
};
pub use environment::{CurrentEnvironment, WHATWGOffscreenKernelEnvironment};
pub use error::JSError;
pub use maplibre::platform::http_client::{
    web_http_client, FetchError, WHATWGFetchHttpClient, WebHttpClient,
};
pub use platform::image_providers;
pub use startup::wasm_bindgen_start;
