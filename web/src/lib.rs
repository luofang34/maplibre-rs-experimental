//! Browser map initialization and worker transports.
#![deny(unused_imports)]

#[cfg(not(any(no_pendantic_os_check, target_arch = "wasm32")))]
compile_error!("web works only on wasm32.");

mod application;
mod environment;
mod error;
mod platform;
mod startup;

pub use application::{run_maplibre, MapType};
pub use environment::WHATWGOffscreenKernelEnvironment;
pub use platform::image_providers;
pub use startup::wasm_bindgen_start;
