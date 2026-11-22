//! Entry points the single-threaded worker script and the page call into this module.

// A panic in a worker entry takes the worker's module down with it; return typed errors.
#![deny(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::{cell::RefCell, rc::Rc};

use futures::future::{Abortable, Aborted};
use maplibre::{
    benchmarking::io::apc::{AsyncProcedure, Input, ProcedureTag},
    environment::{OffscreenKernel, OffscreenKernelConfig},
    io::apc::CallError,
};
#[cfg(feature = "renderer")]
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::DedicatedWorkerGlobalScope;

use self::{running_calls::RunningCalls, worker_styles::WorkerStyles};
#[cfg(any(test, feature = "renderer"))]
use crate::platform::singlethreaded::apc::{decode_call_finished, ReceivedType, WebMessageTag};
use crate::{
    error::{JSError, WebError},
    platform::{
        self,
        singlethreaded::{
            apc::{post_call_finished, set_worker_reply_port},
            PassingContext, UsedContext,
        },
        UsedOffscreenKernelEnvironment,
    },
};

mod running_calls;
mod worker_styles;

thread_local! {
    /// Kept per worker so every call this worker serves shares the active map's asset cache,
    /// while recycled workers cleanly adopt a new map's configuration on reuse.
    static CONFIG: RefCell<Option<OffscreenKernelConfig>> = const { RefCell::new(None) };
}

fn kernel_config() -> OffscreenKernelConfig {
    let mut config = CONFIG
        .with(|slot| slot.borrow().clone())
        .unwrap_or_default();
    // The serialized configuration carries no providers; this worker's own registry does.
    config.image_providers = crate::platform::image_providers().clone();
    config
}

/// Applies the map's serialized [`OffscreenKernelConfig`] inside a tile worker.
pub fn set_kernel_config(config: String) -> Result<(), JSError> {
    let config = maplibre::environment::OffscreenKernelConfig::from_json_slice(config.as_bytes())
        .map_err(WebError::KernelConfigDecoding)?;
    CONFIG.with(|slot| {
        *slot.borrow_mut() = Some(config);
    });
    Ok(())
}

/// Binds a direct `MessagePort` so this tile worker posts replies straight to an `OffscreenCanvas`
/// render worker instead of the UI main thread.
pub fn bind_worker_reply_port(port: Option<web_sys::MessagePort>) {
    set_worker_reply_port(port);
}

fn resolve_procedure(
    procedure_tag: u8,
) -> Result<AsyncProcedure<UsedOffscreenKernelEnvironment, UsedContext>, WebError> {
    match ProcedureTag::from_u8(procedure_tag) {
        Some(ProcedureTag::FetchVector) => Ok(maplibre::vector::fetch_vector_apc::<
            UsedOffscreenKernelEnvironment,
            platform::UsedVectorTransferables,
            _,
        >),
        Some(ProcedureTag::FetchRaster) => Ok(maplibre::raster::fetch_raster_apc::<
            UsedOffscreenKernelEnvironment,
            platform::UsedRasterTransferables,
            _,
        >),
        Some(ProcedureTag::FetchDem) => Ok(maplibre::terrain::fetch_dem_apc::<
            UsedOffscreenKernelEnvironment,
            platform::UsedDemTransferables,
            _,
        >),
        None => {
            #[cfg(test)]
            if procedure_tag == 255 {
                return Ok(tests::never_finishes);
            }
            Err(WebError::UnknownProcedureTag(procedure_tag))
        }
    }
}

/// The tile calls one worker runs, so the host can stop a call it gave up on.
#[derive(Default)]
pub struct TileWorkerCalls {
    running: Rc<RefCell<RunningCalls>>,
    styles: WorkerStyles,
}

impl TileWorkerCalls {
    /// Creates an empty call tracker for a single-threaded tile worker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Keeps the style sent by the host as `revision` for subsequent tile calls naming it.
    pub fn set_style(&mut self, revision: u64, style: String) -> Result<(), JSError> {
        let style =
            maplibre::style::Style::from_json_str(&style).map_err(WebError::StyleDecoding)?;
        #[cfg(all(target_arch = "wasm32", not(test)))]
        if CONFIG.with(|slot| slot.borrow().is_some()) {
            let client = UsedOffscreenKernelEnvironment::create(kernel_config()).source_client();
            maplibre::sdf::assets::spawn_worker_symbol_prefetch(client, &style);
        }
        self.styles.insert(revision, style);
        Ok(())
    }

    /// Starts a call by wire [`ProcedureTag`] with the style kept as `style_revision` and sends
    /// its results back to the host.
    pub fn process(
        &self,
        procedure_tag: u8,
        style_revision: u64,
        input: String,
    ) -> Result<js_sys::Promise, JSError> {
        let procedure = resolve_procedure(procedure_tag)?;

        let mut input = Input::from_json_slice(input.as_bytes()).map_err(|e| {
            log::error!("DeserializeInput error: {e}");
            CallError::DeserializeInput(Box::new(e))
        })?;
        let attempt = input.attempt;
        let context = PassingContext::new();
        let Some(style) = self.styles.get(style_revision) else {
            for kind in [
                maplibre::io::tile_retry::RequestKind::Vector,
                maplibre::io::tile_retry::RequestKind::Raster,
                maplibre::io::tile_retry::RequestKind::Dem,
            ] {
                drop(maplibre::io::apc::Context::send_back(
                    &context,
                    maplibre::io::tile_retry::TileRequestOutcome {
                        coords: input.coords,
                        kind,
                        attempt: Some(attempt),
                        disposition: maplibre::io::tile_retry::RequestDisposition::Retry,
                    },
                ));
            }
            drop(post_call_finished(attempt));
            return Err(WebError::StyleRevisionMissing(style_revision).into());
        };
        input.style = style;

        if let Ok(global) = js_sys::global().dyn_into::<DedicatedWorkerGlobalScope>() {
            let name = global.name();
            log::debug!(
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
        let flush_context = context.clone();
        let call = procedure(
            input,
            context,
            UsedOffscreenKernelEnvironment::create(kernel_config()),
        );
        Ok(wasm_bindgen_futures::future_to_promise(async move {
            let result = Abortable::new(call, registration).await;
            if matches!(result, Ok(Ok(()))) {
                drop(flush_context.flush());
            }
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

    /// Releases host assignment tracking when a call failed before starting its future.
    pub fn finish_failed(&self, attempt: u64) {
        self.running.borrow_mut().finish(attempt);
        drop(post_call_finished(attempt));
    }
}

/// Delivers a `[tag, ArrayBuffer]` message to `received` without leaking raw `Rc` pointers.
#[cfg(any(test, feature = "renderer"))]
pub(crate) fn deliver_received_data(
    received: &ReceivedType,
    tag: u32,
    buffer: js_sys::ArrayBuffer,
) -> Result<(), JSError> {
    let tag = WebMessageTag::from_u32(tag).map_err(|e| CallError::Deserialize(Box::new(e)))?;

    log::debug!(
        "received message ({tag:?}) with {}bytes on main thread",
        buffer.byte_length()
    );

    let waker = {
        let mut received = received
            .try_borrow_mut()
            .map_err(|_| WebError::ReceivedBusy)?;

        if tag == WebMessageTag::CallFinished {
            received.finished.push(decode_call_finished(&buffer)?);
            if !received.messages.is_empty() || !received.queued.is_empty() {
                received.waker.clone()
            } else {
                None
            }
        } else {
            let was_empty = received.messages.is_empty() && received.queued.is_empty();
            crate::platform::singlethreaded::apc::QueuedMessage::push_from_wire(
                &mut received.queued,
                tag,
                buffer,
            )?;
            if was_empty && !received.queued.is_empty() {
                received.waker.clone()
            } else {
                None
            }
        }
    };
    if let Some(wake) = waker {
        wake();
    }

    Ok(())
}

/// Entry point invoked by the main thread. Receives data on the main thread and makes it available
/// to the renderer.
#[cfg(feature = "renderer")]
#[wasm_bindgen]
#[expect(
    unsafe_code,
    reason = "borrows ReceivedType from raw pointer passed by JS worker glue"
)]
pub fn singlethreaded_receive_data(
    received_ptr: *const ReceivedType,
    tag: u32,
    buffer: js_sys::ArrayBuffer,
) -> Result<(), JSError> {
    let received: &ReceivedType = unsafe { &*received_ptr };
    deliver_received_data(received, tag, buffer)
}

#[cfg(test)]
mod tests;
