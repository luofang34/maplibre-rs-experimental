//! Browser map initialization and worker transports.
#![deny(unused_imports)]

#[cfg(not(any(no_pendantic_os_check, target_arch = "wasm32")))]
compile_error!("web works only on wasm32.");

#[cfg(feature = "renderer")]
mod application;
#[cfg(all(feature = "renderer", feature = "arena-telemetry"))]
mod arena_telemetry;
#[cfg(feature = "renderer")]
mod controller;
mod environment;
mod error;
#[cfg(feature = "renderer")]
mod offscreen;
mod platform;
mod startup;

#[cfg(feature = "winit-window")]
pub use application::{
    create_kernel, default_plugins, run_map, run_maplibre, MapPlugin, MapType, WebMapHost,
};
#[cfg(feature = "renderer")]
pub use controller::MapController;
#[cfg(feature = "winit-window")]
pub use environment::CurrentEnvironment;
pub use environment::WHATWGOffscreenKernelEnvironment;
#[cfg(feature = "renderer")]
pub use error::ArgumentError;
pub use error::JSError;
pub use maplibre::platform::http_client::{
    web_http_client, FetchError, WHATWGFetchHttpClient, WebHttpClient,
};
pub use platform::image_providers;
#[cfg(not(target_feature = "atomics"))]
pub use platform::singlethreaded::{
    image_provider::register_image_provider,
    wasm_entries::{bind_worker_reply_port, set_kernel_config, TileWorkerCalls},
};
pub use startup::wasm_bindgen_start;
