//! Render-free single-threaded tile worker WASM entrypoints for `maplibre`.

#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::todo,
        clippy::unimplemented
    )
)]
#![forbid(unsafe_code)]
#![deny(dead_code, unused_imports, missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

#[cfg(not(any(no_pendantic_os_check, target_arch = "wasm32")))]
compile_error!("web-tile-worker works only on wasm32.");

use js_sys::Function;
use wasm_bindgen::prelude::*;
use web_sys::MessagePort;

mod calls;

pub use calls::TileWorkerCalls;

/// Installs logging, panic hook, and shared font registry inside the worker module.
#[wasm_bindgen(start)]
pub fn wasm_bindgen_start() {
    web::wasm_bindgen_start();
}

/// Applies the host's serialized `OffscreenKernelConfig` inside this worker.
#[wasm_bindgen]
pub fn set_kernel_config(config: String) -> Result<(), JsValue> {
    web::set_kernel_config(config).map_err(JsValue::from)
}

/// Binds a direct `MessagePort` so this worker posts replies straight to the render worker.
#[wasm_bindgen]
pub fn bind_worker_reply_port(port: Option<MessagePort>) {
    web::bind_worker_reply_port(port);
}

/// Registers a JavaScript image provider callback for `namespace:...` icon requests.
#[wasm_bindgen]
pub fn register_image_provider(namespace: &str, generation: &str, draw: Function) {
    web::register_image_provider(namespace, generation, draw);
}
