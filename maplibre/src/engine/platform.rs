//! Handles platform specific code. Depending on the compilation target, different
//! parts of this module are used.

#[cfg(all(
    not(target_arch = "wasm32"),
    feature = "native-io",
    feature = "headless",
    test
))]
pub(crate) use noweb::run_current_thread;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-io", feature = "trace"))]
pub use noweb::trace;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-io"))]
pub use noweb::{
    native_http_client, run_multithreaded, NativeHttpClient, ReqwestOffscreenKernelEnvironment,
};

#[cfg(all(not(target_arch = "wasm32"), feature = "native-io"))]
mod noweb;
#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub(crate) use web::image_decode::decode_rgba_bytes;

/// Decodes PNG/JPEG/WebP bytes into RGBA8 pixels off the threads that drive fetches.
pub(crate) async fn decode_rgba(data: Vec<u8>) -> Result<image::RgbaImage, String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        crate::io::cpu_work::run_cpu_bound(move || {
            image::load_from_memory(&data).map(|image| image.to_rgba8())
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
    }
    #[cfg(target_arch = "wasm32")]
    {
        decode_rgba_bytes(&data).await
    }
}

/// The platform's HTTP transport: Reqwest natively (when `native-io` is enabled), `fetch` in the browser.
pub mod http_client {
    #[cfg(all(not(target_arch = "wasm32"), feature = "native-io"))]
    pub use super::noweb::http_client::*;
    #[cfg(target_arch = "wasm32")]
    pub use super::web::http_client::*;
}

/// Scheduler for non-web targets (when `native-io` is enabled).
pub mod scheduler {
    #[cfg(all(not(target_arch = "wasm32"), feature = "native-io"))]
    pub use super::noweb::scheduler::*;
}

pub use crate::util::sync::lock_unpoisoned;

/// Minimum WebGPU buffer size enforced by WebGL.
///
/// This can also be achieved by `_pad` attributes in `shader_ffi.rs`.
pub const MIN_WEBGL_BUFFER_SIZE: u64 = 32;
