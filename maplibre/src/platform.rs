//! Handles platform specific code. Depending on the compilation target, different
//! parts of this module are used.

#[cfg(all(not(target_arch = "wasm32"), feature = "native-io"))]
pub(crate) use noweb::run_cpu_bound;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-io"))]
pub use noweb::run_multithreaded;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-io", feature = "trace"))]
pub use noweb::trace;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-io"))]
pub use noweb::ReqwestOffscreenKernelEnvironment;

/// Runs CPU-bound `work` inline when no native blocking pool is configured.
#[cfg(not(all(not(target_arch = "wasm32"), feature = "native-io")))]
pub(crate) async fn run_cpu_bound<R: Send + 'static>(
    work: impl FnOnce() -> R + Send + 'static,
) -> Result<R, Box<dyn std::error::Error + Send + Sync>> {
    Ok(work())
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-io"))]
mod noweb;
#[cfg(target_arch = "wasm32")]
mod web;

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

/// Minimum WebGPU buffer size
///
/// FIXME: This limit is enforced by WebGL. Actually this makes sense!
/// FIXME: This can also be achieved by _pad attributes in shader_ffi.rs
pub const MIN_WEBGL_BUFFER_SIZE: u64 = 32;
