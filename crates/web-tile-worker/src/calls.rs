//! `TileWorkerCalls` JS binding wrapper for the render-free worker WASM binary.

use js_sys::Promise;
use wasm_bindgen::prelude::*;

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

    /// Runs a tile procedure directly from packed scalar tile fields without JSON parsing.
    pub fn process_tile(
        &self,
        attempt: u64,
        style_revision: u64,
        x: i32,
        y: i32,
        meta: u32,
        pixel_ratio: f32,
    ) -> Result<Promise, JsValue> {
        self.inner
            .process_tile(attempt, style_revision, x, y, meta, pixel_ratio)
            .map_err(JsValue::from)
    }

    /// Cancels an active tile call by `attempt`; returns `true` if it was still running.
    pub fn cancel(&self, attempt: u64) -> bool {
        self.inner.cancel(attempt)
    }

    /// Cancels a batch of active tile calls by `attempt`.
    pub fn cancel_batch(&self, attempts: &[u64]) {
        self.inner.cancel_batch(attempts);
    }

    /// Releases host tracking when a tile call failed before its async future started.
    pub fn finish_failed(&self, attempt: u64) {
        self.inner.finish_failed(attempt);
    }
}
