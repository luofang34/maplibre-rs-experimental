//! Main-thread transport for tile calls served by single-threaded web workers.

// A failure here must reach the map as a typed error: a panic on the render thread stops it.
#![deny(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::{cell::RefCell, rc::Rc, vec::IntoIter};

use js_sys::{ArrayBuffer, Uint8Array};
use maplibre::{
    environment::{OffscreenKernel, OffscreenKernelConfig},
    io::{
        apc::{
            AsyncProcedure, AsyncProcedureCall, CallError, Context, Input, IntoMessage, Message,
            MessageTag, SendError,
        },
        scheduler::ScheduleError,
        source_client::SourceClient,
        tile_retry::{RequestKind, TileRequestOutcome},
    },
    sdf::provided::ProvidedImagesReport,
};
use rand::{thread_rng, Rng};
use thiserror::Error;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{DedicatedWorkerGlobalScope, Worker};

use self::{assignments::Assignments, style_revisions::StyleRevisions};
use crate::{
    error::WebError,
    platform::singlethreaded::{
        transferables::FlatBufferTransferable, UsedContext, UsedHttpClient,
    },
};

/// Error which happens during serialization or deserialization of the tag
#[derive(Error, Debug)]
#[error("failed to deserialize message tag")]
pub struct MessageTagDeserializeError;

impl MessageTag for WebMessageTag {
    fn dyn_clone(&self) -> Box<dyn MessageTag> {
        Box::new(*self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum WebMessageTag {
    TileTessellated = 1,
    LayerMissing = 2,
    LayerTessellated = 3,
    LayerIndexed = 4,
    LayerRaster = 5,
    LayerRasterMissing = 6,
    SymbolLayerTessellated = 7,
    LayerDem = 8,
    LayerDemMissing = 9,
    TileRequestOutcome = 10,
    TrackedPayload = 11,
    ProvidedImages = 12,
    /// A worker stopped running the call for the attempt in the payload, either way.
    CallFinished = 13,
}

impl WebMessageTag {
    pub fn to_static(self) -> &'static WebMessageTag {
        match self {
            WebMessageTag::LayerRaster => &WebMessageTag::LayerRaster,
            WebMessageTag::LayerMissing => &WebMessageTag::LayerMissing,
            WebMessageTag::LayerIndexed => &WebMessageTag::LayerIndexed,
            WebMessageTag::TileTessellated => &WebMessageTag::TileTessellated,
            WebMessageTag::LayerTessellated => &WebMessageTag::LayerTessellated,
            WebMessageTag::LayerRasterMissing => &WebMessageTag::LayerRasterMissing,
            WebMessageTag::SymbolLayerTessellated => &WebMessageTag::SymbolLayerTessellated,
            WebMessageTag::LayerDem => &WebMessageTag::LayerDem,
            WebMessageTag::LayerDemMissing => &WebMessageTag::LayerDemMissing,
            WebMessageTag::TileRequestOutcome => &WebMessageTag::TileRequestOutcome,
            WebMessageTag::TrackedPayload => &WebMessageTag::TrackedPayload,
            WebMessageTag::ProvidedImages => &WebMessageTag::ProvidedImages,
            WebMessageTag::CallFinished => &WebMessageTag::CallFinished,
        }
    }

    pub fn from_u32(tag: u32) -> Result<Self, MessageTagDeserializeError> {
        match tag {
            x if x == WebMessageTag::LayerMissing as u32 => Ok(WebMessageTag::LayerMissing),
            x if x == WebMessageTag::LayerTessellated as u32 => Ok(WebMessageTag::LayerTessellated),
            x if x == WebMessageTag::TileTessellated as u32 => Ok(WebMessageTag::TileTessellated),
            x if x == WebMessageTag::LayerIndexed as u32 => Ok(WebMessageTag::LayerIndexed),
            x if x == WebMessageTag::LayerRaster as u32 => Ok(WebMessageTag::LayerRaster),
            x if x == WebMessageTag::LayerRasterMissing as u32 => {
                Ok(WebMessageTag::LayerRasterMissing)
            }
            x if x == WebMessageTag::SymbolLayerTessellated as u32 => {
                Ok(WebMessageTag::SymbolLayerTessellated)
            }
            x if x == WebMessageTag::LayerDem as u32 => Ok(WebMessageTag::LayerDem),
            x if x == WebMessageTag::LayerDemMissing as u32 => Ok(WebMessageTag::LayerDemMissing),
            x if x == WebMessageTag::TileRequestOutcome as u32 => {
                Ok(WebMessageTag::TileRequestOutcome)
            }
            x if x == WebMessageTag::TrackedPayload as u32 => Ok(WebMessageTag::TrackedPayload),
            x if x == WebMessageTag::ProvidedImages as u32 => Ok(WebMessageTag::ProvidedImages),
            x if x == WebMessageTag::CallFinished as u32 => Ok(WebMessageTag::CallFinished),
            _ => Err(MessageTagDeserializeError),
        }
    }
}

impl From<WebMessageTag> for u32 {
    fn from(val: WebMessageTag) -> Self {
        val as u32
    }
}

#[derive(Clone)]
pub struct PassingContext {
    pub source_client: SourceClient<UsedHttpClient>,
}

impl Context for PassingContext {
    fn send_back<T: IntoMessage>(&self, message: T) -> Result<(), SendError> {
        let (tag, buffer) = prepare_message(message.into())?;
        let global: DedicatedWorkerGlobalScope =
            js_sys::global()
                .dyn_into()
                .map_err(|_| SendError::Transmission {
                    operation: "accessing the worker global scope",
                    source: Box::new(WebError::TypeError(
                        "not a dedicated worker global scope".into(),
                    )),
                })?;
        global
            .post_message_with_transfer(
                &js_sys::Array::of2(&JsValue::from(tag as u32), &buffer),
                &js_sys::Array::of1(&buffer),
            )
            .map_err(|source| SendError::Transmission {
                operation: "posting a worker result",
                source: Box::new(WebError::from(source)),
            })
    }
}

fn prepare_message(message: Message) -> Result<(WebMessageTag, ArrayBuffer), SendError> {
    let attempt = message.attempt();
    let (tag, buffer) = prepare_payload(message)?;
    match attempt {
        Some(attempt) => {
            let mut bytes = Vec::with_capacity(12 + buffer.byte_length() as usize);
            bytes.extend_from_slice(&(tag as u32).to_le_bytes());
            bytes.extend_from_slice(&attempt.to_le_bytes());
            bytes.extend_from_slice(&Uint8Array::new(&buffer).to_vec());
            Ok((
                WebMessageTag::TrackedPayload,
                Uint8Array::from(bytes.as_slice()).buffer(),
            ))
        }
        None => Ok((tag, buffer)),
    }
}

fn prepare_payload(message: Message) -> Result<(WebMessageTag, ArrayBuffer), SendError> {
    if message.has_tag(RequestKind::Raster.message_tag())
        || message.has_tag(RequestKind::Dem.message_tag())
        || message.has_tag(RequestKind::Vector.message_tag())
    {
        let outcome = message.into_transferable::<TileRequestOutcome>()?;
        let data = serde_json::to_vec(&*outcome).map_err(|source| SendError::Transmission {
            operation: "encoding a tile request outcome",
            source: Box::new(source),
        })?;
        return Ok((
            WebMessageTag::TileRequestOutcome,
            Uint8Array::from(data.as_slice()).buffer(),
        ));
    }
    if message.has_tag(ProvidedImagesReport::message_tag()) {
        let report = message.into_transferable::<ProvidedImagesReport>()?;
        let data = serde_json::to_vec(&*report).map_err(|source| SendError::Transmission {
            operation: "encoding a provided images report",
            source: Box::new(source),
        })?;
        return Ok((
            WebMessageTag::ProvidedImages,
            Uint8Array::from(data.as_slice()).buffer(),
        ));
    }
    let tag = message
        .tag()
        .as_any()
        .downcast_ref::<WebMessageTag>()
        .copied()
        .ok_or(SendError::UnsupportedTag { tag: message.tag() })?;
    let transferable = message.into_transferable::<FlatBufferTransferable>()?;
    let data = transferable.data();
    let buffer = Uint8Array::from(data).buffer();
    tracing::debug!(?tag, bytes = data.len(), "sending worker result");
    Ok((tag, buffer))
}

pub(crate) fn decode_message(
    tag: WebMessageTag,
    buffer: ArrayBuffer,
) -> Result<Message, CallError> {
    if tag == WebMessageTag::TrackedPayload {
        let bytes = Uint8Array::new(&buffer).to_vec();
        let header = bytes.first_chunk::<12>().ok_or_else(|| {
            CallError::Deserialize(Box::new(TrackedPayloadError::Header { bytes: bytes.len() }))
        })?;
        let tag = WebMessageTag::from_u32(u32::from_le_bytes([
            header[0], header[1], header[2], header[3],
        ]))
        .map_err(|error| CallError::Deserialize(Box::new(error)))?;
        if tag == WebMessageTag::TrackedPayload {
            return Err(CallError::Deserialize(Box::new(
                TrackedPayloadError::Nested,
            )));
        }
        let attempt = u64::from_le_bytes([
            header[4], header[5], header[6], header[7], header[8], header[9], header[10],
            header[11],
        ]);
        let payload = Uint8Array::from(&bytes[12..]).buffer();
        return decode_message(tag, payload).map(|message| message.with_attempt(attempt));
    }
    if tag == WebMessageTag::TileRequestOutcome {
        let data = Uint8Array::new(&buffer).to_vec();
        let outcome: TileRequestOutcome = serde_json::from_slice(&data)
            .map_err(|source| CallError::Deserialize(Box::new(source)))?;
        return Ok(IntoMessage::into(outcome));
    }
    if tag == WebMessageTag::ProvidedImages {
        let data = Uint8Array::new(&buffer).to_vec();
        let report: ProvidedImagesReport = serde_json::from_slice(&data)
            .map_err(|source| CallError::Deserialize(Box::new(source)))?;
        return Ok(IntoMessage::into(report));
    }
    Ok(Message::new(
        tag.to_static(),
        Box::new(FlatBufferTransferable::from_array_buffer(tag, buffer)),
    ))
}

/// Encodes the payload of a [`WebMessageTag::CallFinished`] message.
pub(crate) fn encode_call_finished(attempt: u64) -> ArrayBuffer {
    Uint8Array::from(attempt.to_le_bytes().as_slice()).buffer()
}

/// Reads the attempt from the payload of a [`WebMessageTag::CallFinished`] message.
pub(crate) fn decode_call_finished(buffer: &ArrayBuffer) -> Result<u64, CallError> {
    let bytes = Uint8Array::new(buffer).to_vec();
    let attempt = <[u8; 8]>::try_from(bytes.as_slice()).map_err(|_| {
        CallError::Deserialize(Box::new(TrackedPayloadError::Finished {
            bytes: bytes.len(),
        }))
    })?;
    Ok(u64::from_le_bytes(attempt))
}

/// Tells the main thread this worker stopped running the call for `attempt`.
pub(crate) fn post_call_finished(attempt: u64) -> Result<(), SendError> {
    let global: DedicatedWorkerGlobalScope =
        js_sys::global()
            .dyn_into()
            .map_err(|_| SendError::Transmission {
                operation: "accessing the worker global scope",
                source: Box::new(WebError::TypeError(
                    "not a dedicated worker global scope".into(),
                )),
            })?;
    let buffer = encode_call_finished(attempt);
    global
        .post_message_with_transfer(
            &js_sys::Array::of2(&JsValue::from(WebMessageTag::CallFinished as u32), &buffer),
            &js_sys::Array::of1(&buffer),
        )
        .map_err(|source| SendError::Transmission {
            operation: "posting a finished call",
            source: Box::new(WebError::from(source)),
        })
}

#[derive(Debug, Error)]
enum TrackedPayloadError {
    #[error("tracked worker payload has {bytes} bytes; its header requires 12")]
    Header { bytes: usize },
    #[error("tracked worker payload contains a nested tracking envelope")]
    Nested,
    #[error("finished call payload has {bytes} bytes; its attempt requires 8")]
    Finished { bytes: usize },
}

/// What the workers sent since the last receive.
#[derive(Default)]
pub struct Received {
    /// Results, in delivery order.
    pub(crate) messages: Vec<Message>,
    /// Attempts whose calls a worker stopped running.
    pub(crate) finished: Vec<u64>,
}

pub type ReceivedType = RefCell<Received>;

pub struct PassingAsyncProcedureCall {
    workers: Vec<Worker>,

    buffer: RefCell<Vec<Message>>,

    received: Rc<ReceivedType>, // FIXME: Is RefCell fine?

    /// The worker each running call was sent to, so a cancel reaches it.
    assignments: RefCell<Assignments>,

    /// The style the workers hold, so calls name it instead of carrying it.
    styles: RefCell<StyleRevisions>,
}

impl PassingAsyncProcedureCall {
    pub fn new(
        new_worker: js_sys::Function,
        initial_workers: usize,
        config: OffscreenKernelConfig,
    ) -> Result<Self, WebError> {
        let received = Rc::new(RefCell::new(Received::default()));

        let create_new_worker = || {
            new_worker
                .call1(
                    &JsValue::undefined(),
                    &JsValue::from(Rc::into_raw(received.clone())),
                )
                .map_err(WebError::from)?
                .dyn_into::<Worker>()
                .map_err(|_e| WebError::TypeError("Unable to cast to Worker".into()))
        };

        let mut workers = Vec::with_capacity(initial_workers);
        let config = serde_json::to_string(&config).map_err(WebError::KernelConfigEncoding)?;
        let init = worker_message("wasm_init", &[("module", &wasm_bindgen::module())])?;
        let configure = worker_message("kernel_config", &[("config", &JsValue::from(config))])?;

        for _ in 0..initial_workers {
            let worker: Worker = create_new_worker()?;
            worker.post_message(&init).map_err(WebError::from)?;
            worker.post_message(&configure).map_err(WebError::from)?;
            workers.push(worker);
        }

        Ok(Self {
            workers,
            buffer: RefCell::new(Vec::default()),
            received,
            assignments: RefCell::default(),
            styles: RefCell::default(),
        })
    }
}

/// Builds a worker message `{ type: kind, <key>: value, ... }`.
fn worker_message(kind: &str, fields: &[(&str, &JsValue)]) -> Result<js_sys::Object, WebError> {
    let entries = js_sys::Array::of1(&js_sys::Array::of2(
        &JsValue::from("type"),
        &JsValue::from(kind),
    ));
    for (key, value) in fields {
        entries.push(&js_sys::Array::of2(&JsValue::from(*key), value));
    }
    js_sys::Object::from_entries(&entries).map_err(WebError::from)
}

impl<K: OffscreenKernel> AsyncProcedureCall<K> for PassingAsyncProcedureCall {
    type Context = UsedContext;
    type ReceiveIterator<F: FnMut(&Message) -> bool> = IntoIter<Message>;

    fn receive<F: FnMut(&Message) -> bool>(&self, mut filter: F) -> Self::ReceiveIterator<F> {
        let mut buffer = self.buffer.borrow_mut();
        // Partial and final tile completions must retain the worker's delivery order.
        let mut ret: Vec<_> = buffer.extract_if(.., |message| filter(message)).collect();

        // TODO: (optimize) Using while instead of if means that we are processing all that is
        // TODO available this might cause frame drops.
        let Ok(mut received) = self.received.try_borrow_mut() else {
            return ret.into_iter();
        };
        let mut assignments = self.assignments.borrow_mut();
        for attempt in received.finished.drain(..) {
            assignments.finished(attempt);
        }
        for message in received.messages.drain(..) {
            log::debug!("Data reached main thread: {message:?}");

            if filter(&message) {
                ret.push(message);
            } else {
                buffer.push(message)
            }
        }

        ret.into_iter()
    }

    fn call(
        &self,
        input: Input,
        procedure: AsyncProcedure<K, UsedContext>,
    ) -> Result<(), CallError> {
        let procedure_ptr = procedure as *mut AsyncProcedure<K, UsedContext>; // TODO: Verify how these pointer are converted to pointers
        let attempt = input.attempt;
        if self.workers.is_empty() {
            return Err(CallError::Schedule(ScheduleError::NoWorkers));
        }
        // The input leaves its style out; the workers hold it, sent once per revision.
        let style_revision = self
            .styles
            .borrow_mut()
            .revision(&input.style, |revision, style| {
                let message = worker_message(
                    "style",
                    &[
                        ("revision", &JsValue::from(revision)),
                        ("style", &JsValue::from(style)),
                    ],
                )?;
                self.workers
                    .iter()
                    .try_for_each(|worker| worker.post_message(&message).map_err(WebError::from))
            })
            .map_err(|error| CallError::Serialize(Box::new(error)))?;
        let input = serde_json::to_string(&input).map_err(|e| CallError::Serialize(Box::new(e)))?;

        // The worker reads the attempt without decoding the input, so a cancel can overtake
        // a call that waits for the worker to start.
        let message = worker_message(
            "call",
            &[
                ("procedure_ptr", &JsValue::from(procedure_ptr)),
                ("attempt", &JsValue::from(attempt)),
                ("style_revision", &JsValue::from(style_revision)),
                ("input", &JsValue::from(input)),
            ],
        )
        .map_err(|error| CallError::Serialize(Box::new(error)))?;

        let index = thread_rng().gen_range(0..self.workers.len());
        let worker = self
            .workers
            .get(index)
            .ok_or(CallError::Schedule(ScheduleError::NoWorkers))?;

        worker.post_message(&message).map_err(|error| {
            CallError::Schedule(ScheduleError::Scheduling(Box::new(WebError::from(error))))
        })?;
        self.assignments.borrow_mut().assign(attempt, index);
        Ok(())
    }

    fn cancel(&self, attempt: u64) {
        let Some(index) = self.assignments.borrow_mut().cancel(attempt) else {
            return;
        };
        let Some(worker) = self.workers.get(index) else {
            return;
        };
        let posted = worker_message("cancel", &[("attempt", &JsValue::from(attempt))])
            .and_then(|message| worker.post_message(&message).map_err(WebError::from));
        if let Err(error) = posted {
            // The call finishes and the map ignores its result.
            tracing::warn!(attempt, %error, "cancelling a worker call failed");
        }
    }
}

mod assignments;
mod style_revisions;
#[cfg(test)]
mod tests;
