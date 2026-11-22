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

use js_sys::{Function, Promise};
use wasm_bindgen::prelude::*;
use web_sys::MessagePort;

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

/// Single-threaded tile-call dispatcher and cancellation registry for one worker.
#[wasm_bindgen]
#[derive(Default)]
pub struct TileWorkerCalls {
    inner: web::TileWorkerCalls,
}

#[wasm_bindgen]
impl TileWorkerCalls {
    /// Creates an empty tile-call tracker.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores the parsed style JSON under `revision` for subsequent tile calls.
    pub fn set_style(
        &mut self,
        revision: u64,
        style: String,
        prefetch: Option<bool>,
    ) -> Result<(), JsValue> {
        self.inner
            .set_style(revision, style, prefetch)
            .map_err(JsValue::from)
    }

    /// Runs a tile procedure by wire `ProcedureTag` byte using `style_revision`.
    pub fn process(
        &self,
        procedure_tag: u8,
        style_revision: u64,
        input: String,
    ) -> Result<Promise, JsValue> {
        self.inner
            .process(procedure_tag, style_revision, input)
            .map_err(JsValue::from)
    }

    /// Cancels an active tile call by `attempt`; returns `true` if it was still running.
    pub fn cancel(&self, attempt: u64) -> bool {
        self.inner.cancel(attempt)
    }

    /// Releases host tracking when a tile call failed before its async future started.
    pub fn finish_failed(&self, attempt: u64) {
        self.inner.finish_failed(attempt);
    }
}
