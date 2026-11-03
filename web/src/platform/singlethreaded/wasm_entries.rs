//! Entry points the single-threaded worker script and the page call into this module.

// A panic in a worker entry takes the worker's module down with it; return typed errors.
#![deny(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::{cell::RefCell, mem, rc::Rc, sync::OnceLock};

use futures::future::{Abortable, Aborted};
use maplibre::{
    benchmarking::io::{
        apc::{AsyncProcedure, Input},
        source_client::{HttpSourceClient, SourceClient},
    },
    environment::{OffscreenKernel, OffscreenKernelConfig},
    io::apc::CallError,
};
use wasm_bindgen::prelude::*;
use web_sys::DedicatedWorkerGlobalScope;

use self::{running_calls::RunningCalls, worker_styles::WorkerStyles};
use crate::{
    error::{JSError, WebError},
    platform::{
        http_client::web_http_client,
        singlethreaded::{
            apc::{decode_call_finished, post_call_finished, ReceivedType, WebMessageTag},
            PassingContext, UsedContext,
        },
        UsedOffscreenKernelEnvironment,
    },
};

mod running_calls;
mod worker_styles;

/// Parsed once so every call this worker serves shares the configuration's asset cache.
static CONFIG: OnceLock<OffscreenKernelConfig> = OnceLock::new();

fn kernel_config() -> OffscreenKernelConfig {
    let mut config = CONFIG.get().cloned().unwrap_or_default();
    // The serialized configuration carries no providers; this worker's own registry does.
    config.image_providers = crate::platform::image_providers().clone();
    config
}

#[wasm_bindgen]
pub fn set_kernel_config(config: String) -> Result<(), JSError> {
    let config = serde_json::from_str(&config).map_err(WebError::KernelConfigDecoding)?;
    CONFIG
        .set(config)
        .map_err(|_| WebError::KernelConfigAlreadySet)?;
    Ok(())
}

/// The tile calls one worker runs, so the main thread can stop a call it gave up on.
#[wasm_bindgen]
#[derive(Default)]
pub struct TileWorkerCalls {
    running: Rc<RefCell<RunningCalls>>,
    styles: WorkerStyles,
}

#[wasm_bindgen]
impl TileWorkerCalls {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Keeps the style the main thread sent as `revision` for the calls that name it.
    pub fn set_style(&mut self, revision: u64, style: String) -> Result<(), JSError> {
        let style = serde_json::from_str(&style).map_err(WebError::StyleDecoding)?;
        self.styles.insert(revision, style);
        Ok(())
    }

    /// Starts a call with the style kept as `style_revision` and sends its results back to the
    /// main thread. The call is registered before this returns, so a cancel that arrives later
    /// stops it; the promise settles when the call ends, whether it finished or was stopped.
    pub fn process(
        &self,
        procedure_ptr: u32,
        style_revision: u64,
        input: String,
    ) -> Result<js_sys::Promise, JSError> {
        // SAFETY: the main thread sends the address of an `AsyncProcedure` of this same module.
        let procedure: AsyncProcedure<UsedOffscreenKernelEnvironment, UsedContext> =
            unsafe { mem::transmute(procedure_ptr) };

        let mut input = serde_json::from_str::<Input>(&input).map_err(|e| {
            log::error!("DeserializeInput error: {e}");
            CallError::DeserializeInput(Box::new(e))
        })?;
        let attempt = input.attempt;
        let context = PassingContext {
            source_client: SourceClient::new(HttpSourceClient::new(web_http_client())),
        };
        let Some(style) = self.styles.get(style_revision) else {
            for kind in [
                maplibre::io::tile_retry::RequestKind::Vector,
                maplibre::io::tile_retry::RequestKind::Raster,
                maplibre::io::tile_retry::RequestKind::Dem,
            ] {
                let _ = maplibre::io::apc::Context::send_back(
                    &context,
                    maplibre::io::tile_retry::TileRequestOutcome {
                        coords: input.coords,
                        kind,
                        attempt: Some(attempt),
                        disposition: maplibre::io::tile_retry::RequestDisposition::Retry,
                    },
                );
            }
            let _ = post_call_finished(attempt);
            return Err(WebError::StyleRevisionMissing(style_revision).into());
        };
        input.style = style;

        if let Ok(global) = js_sys::global().dyn_into::<DedicatedWorkerGlobalScope>() {
            let name = global.name();
            log::info!(
                "Processing on web worker: {}",
                if name.is_empty() {
                    "name not set"
                } else {
                    name.as_str()
                }
            );
        }

        let registration = self.running.borrow_mut().start(attempt);
        let running = self.running.clone();
        let call = procedure(
            input,
            context,
            UsedOffscreenKernelEnvironment::create(kernel_config()),
        );
        Ok(wasm_bindgen_futures::future_to_promise(async move {
            let result = Abortable::new(call, registration).await;
            running.borrow_mut().finish(attempt);
            if let Err(error) = post_call_finished(attempt) {
                tracing::warn!(attempt, %error, "reporting a finished call failed");
            }
            match result {
                Ok(Ok(())) => Ok(JsValue::UNDEFINED),
                Err(Aborted) => {
                    log::debug!("cancelled worker call {attempt} stopped");
                    Ok(JsValue::UNDEFINED)
                }
                Ok(Err(error)) => Err(JSError::from(error).into()),
            }
        }))
    }

    /// Stops the call for `attempt` at its next await; `false` when it is not running.
    pub fn cancel(&self, attempt: u64) -> bool {
        self.running.borrow_mut().cancel(attempt)
    }

    /// Releases main-thread assignment tracking when a call failed before starting its future.
    pub fn finish_failed(&self, attempt: u64) {
        self.running.borrow_mut().finish(attempt);
        let _ = post_call_finished(attempt);
    }
}

/// Entry point invoked by the main thread. Receives data on the main thread and makes it available
/// to the renderer.
#[wasm_bindgen]
pub fn singlethreaded_receive_data(
    received_ptr: *const ReceivedType,
    tag: u32,
    buffer: js_sys::ArrayBuffer,
) -> Result<(), JSError> {
    let tag = WebMessageTag::from_u32(tag).map_err(|e| CallError::Deserialize(Box::new(e)))?;

    log::debug!(
        "received message ({tag:?}) with {}bytes on main thread",
        buffer.byte_length()
    );

    // SAFETY: the pointer comes from `Rc::into_raw` in `PassingAsyncProcedureCall::new`, whose
    // count is never released, so the value outlives every worker message. Borrowing it leaves
    // that count alone.
    let received: &ReceivedType = unsafe { &*received_ptr };
    let mut received = received
        .try_borrow_mut()
        .map_err(|_| WebError::ReceivedBusy)?;

    if tag == WebMessageTag::CallFinished {
        received.finished.push(decode_call_finished(&buffer)?);
    } else {
        received
            .messages
            .push(super::apc::decode_message(tag, buffer)?);
    }

    Ok(())
}

#[cfg(test)]
mod tests;
