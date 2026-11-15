//! Handles platform specific code. Depending on the compilation target, different
//! parts of this module are used.

#[cfg(all(not(target_arch = "wasm32"), feature = "native-io"))]
pub(crate) use noweb::run_cpu_bound;
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
#[cfg(target_arch = "wasm32")]
pub(crate) use web::image_decode::decode_rgba_bytes;

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

/// Acquires a [`std::sync::Mutex`] without calling `memory.atomic.wait32` on `wasm32 + atomics`
/// (where the browser main thread forbids `Atomics.wait`).
pub fn lock_unpoisoned<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
    {
        loop {
            match mutex.try_lock() {
                Ok(guard) => return guard,
                Err(std::sync::TryLockError::Poisoned(poisoned)) => return poisoned.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => core::hint::spin_loop(),
            }
        }
    }
    #[cfg(not(all(target_arch = "wasm32", target_feature = "atomics")))]
    {
        mutex
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Acquires a read guard on [`std::sync::RwLock`] without `Atomics.wait` on `wasm32 + atomics`.
pub(crate) fn read_unpoisoned<T>(lock: &std::sync::RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
    {
        loop {
            match lock.try_read() {
                Ok(guard) => return guard,
                Err(std::sync::TryLockError::Poisoned(poisoned)) => return poisoned.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => core::hint::spin_loop(),
            }
        }
    }
    #[cfg(not(all(target_arch = "wasm32", target_feature = "atomics")))]
    {
        lock.read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Acquires a write guard on [`std::sync::RwLock`] without `Atomics.wait` on `wasm32 + atomics`.
pub(crate) fn write_unpoisoned<T>(
    lock: &std::sync::RwLock<T>,
) -> std::sync::RwLockWriteGuard<'_, T> {
    #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
    {
        loop {
            match lock.try_write() {
                Ok(guard) => return guard,
                Err(std::sync::TryLockError::Poisoned(poisoned)) => return poisoned.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => core::hint::spin_loop(),
            }
        }
    }
    #[cfg(not(all(target_arch = "wasm32", target_feature = "atomics")))]
    {
        lock.write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Minimum WebGPU buffer size
///
/// FIXME: This limit is enforced by WebGL. Actually this makes sense!
/// FIXME: This can also be achieved by _pad attributes in shader_ffi.rs
pub const MIN_WEBGL_BUFFER_SIZE: u64 = 32;
