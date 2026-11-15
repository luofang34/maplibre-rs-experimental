//! Wire encoding, per-tile message coalescing, and decoding for single-threaded web workers.

#![deny(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::{
    cell::RefCell,
    sync::{Arc, Mutex, MutexGuard},
};

use js_sys::{ArrayBuffer, Uint8Array};
use maplibre::{
    io::{
        apc::{CallError, Context, IntoMessage, Message, SendError},
        source_client::SourceClient,
        tile_retry::{RequestKind, TileRequestOutcome},
    },
    sdf::provided::ProvidedImagesReport,
};
use thiserror::Error;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{DedicatedWorkerGlobalScope, MessagePort};

use super::WebMessageTag;
use crate::{
    error::WebError,
    platform::singlethreaded::{transferables::FlatBufferTransferable, UsedHttpClient},
};

pub(crate) const BATCH_ITEM_HEADER_BYTES: usize = 20;

thread_local! {
    static REPLY_PORT: RefCell<Option<MessagePort>> = const { RefCell::new(None) };
}

/// Binds a direct `MessagePort` for tile worker replies (used when a dedicated `OffscreenCanvas`
/// render worker talks directly to tile workers without routing through the UI main thread).
pub(crate) fn set_worker_reply_port(port: Option<MessagePort>) {
    REPLY_PORT.with(|slot| {
        *slot.borrow_mut() = port;
    });
}

struct BufferedItem {
    tag: WebMessageTag,
    attempt: Option<u64>,
    payload: PayloadBytes,
}

#[derive(Clone)]
pub struct PassingContext {
    pub source_client: SourceClient<UsedHttpClient>,
    pending: Arc<Mutex<Vec<BufferedItem>>>,
}

impl PassingContext {
    pub fn new(source_client: SourceClient<UsedHttpClient>) -> Self {
        Self {
            source_client,
            pending: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub(crate) fn flush(&self) -> Result<(), SendError> {
        let items = std::mem::take(&mut *lock_pending(&self.pending));
        if items.is_empty() {
            return Ok(());
        }
        let (wire_tag, buffer) = if items.len() == 1 {
            let mut iter = items.into_iter();
            let Some(item) = iter.next() else {
                return Ok(());
            };
            encode_single_item(item.tag, item.attempt, &item.payload)
        } else {
            (WebMessageTag::BatchPayload, encode_batch_items(&items))
        };
        post_wire_message(wire_tag, &buffer)
    }
}

fn lock_pending<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn worker_global_scope() -> Result<DedicatedWorkerGlobalScope, SendError> {
    js_sys::global()
        .dyn_into()
        .map_err(|_| SendError::Transmission {
            operation: "accessing the worker global scope",
            source: Box::new(WebError::TypeError(
                "not a dedicated worker global scope".into(),
            )),
        })
}

fn post_wire_message(tag: WebMessageTag, buffer: &ArrayBuffer) -> Result<(), SendError> {
    let payload = js_sys::Array::of2(&JsValue::from(tag as u32), buffer);
    let transfer = js_sys::Array::of1(buffer);
    let posted = REPLY_PORT.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|port| port.post_message_with_transferable(&payload, &transfer))
    });
    match posted {
        Some(res) => res.map_err(|source| SendError::Transmission {
            operation: "posting a worker result over MessagePort",
            source: Box::new(WebError::from(source)),
        }),
        None => worker_global_scope()?
            .post_message_with_transfer(&payload, &transfer)
            .map_err(|source| SendError::Transmission {
                operation: "posting a worker result",
                source: Box::new(WebError::from(source)),
            }),
    }
}

impl Context for PassingContext {
    fn send_back<T: IntoMessage>(&self, message: T) -> Result<(), SendError> {
        let message = message.into();
        let attempt = message.attempt();
        let (tag, payload) = prepare_payload_bytes(message)?;
        if tag.is_intermediate_layer() {
            lock_pending(&self.pending).push(BufferedItem {
                tag,
                attempt,
                payload,
            });
            return Ok(());
        }
        let mut items = std::mem::take(&mut *lock_pending(&self.pending));
        if items.is_empty() {
            let (wire_tag, buffer) = encode_single_item(tag, attempt, &payload);
            drop(payload);
            return post_wire_message(wire_tag, &buffer);
        }
        items.push(BufferedItem {
            tag,
            attempt,
            payload,
        });
        let buffer = encode_batch_items(&items);
        drop(items);
        post_wire_message(WebMessageTag::BatchPayload, &buffer)
    }
}

#[cfg(test)]
pub(crate) fn prepare_message(message: Message) -> Result<(WebMessageTag, ArrayBuffer), SendError> {
    let attempt = message.attempt();
    let (tag, payload) = prepare_payload_bytes(message)?;
    Ok(encode_single_item(tag, attempt, &payload))
}

fn encode_single_item(
    tag: WebMessageTag,
    attempt: Option<u64>,
    payload: &PayloadBytes,
) -> (WebMessageTag, ArrayBuffer) {
    match attempt {
        Some(attempt) => {
            let payload_slice = payload.as_slice();
            let total_len = 12 + payload_slice.len();
            let out = Uint8Array::new_with_length(total_len as u32);
            let mut header = [0u8; 12];
            header[0..4].copy_from_slice(&(tag as u32).to_le_bytes());
            header[4..12].copy_from_slice(&attempt.to_le_bytes());
            out.subarray(0, 12).copy_from(&header);
            if !payload_slice.is_empty() {
                out.subarray(12, total_len as u32).copy_from(payload_slice);
            }
            (WebMessageTag::TrackedPayload, out.buffer())
        }
        None => (tag, Uint8Array::from(payload.as_slice()).buffer()),
    }
}

fn encode_batch_items(items: &[BufferedItem]) -> ArrayBuffer {
    let total_len: usize = items
        .iter()
        .map(|item| BATCH_ITEM_HEADER_BYTES + item.payload.as_slice().len())
        .sum();
    let out = Uint8Array::new_with_length(total_len as u32);
    let mut cursor = 0_u32;
    for item in items {
        let slice = item.payload.as_slice();
        let payload_len = slice.len() as u32;
        let mut header = [0_u8; BATCH_ITEM_HEADER_BYTES];
        header[0..4].copy_from_slice(&(item.tag as u32).to_le_bytes());
        header[4..8].copy_from_slice(&u32::from(item.attempt.is_some()).to_le_bytes());
        header[8..16].copy_from_slice(&item.attempt.unwrap_or(0).to_le_bytes());
        header[16..20].copy_from_slice(&payload_len.to_le_bytes());
        let header_end = cursor + BATCH_ITEM_HEADER_BYTES as u32;
        out.subarray(cursor, header_end).copy_from(&header);
        if payload_len > 0 {
            out.subarray(header_end, header_end + payload_len)
                .copy_from(slice);
        }
        cursor = header_end + payload_len;
    }
    out.buffer()
}

enum PayloadBytes {
    Vec(Vec<u8>),
    Transferable(Box<FlatBufferTransferable>),
}

impl PayloadBytes {
    fn as_slice(&self) -> &[u8] {
        match self {
            Self::Vec(bytes) => bytes.as_slice(),
            Self::Transferable(transferable) => transferable.data(),
        }
    }
}

fn prepare_payload_bytes(message: Message) -> Result<(WebMessageTag, PayloadBytes), SendError> {
    if message.has_tag(RequestKind::Raster.message_tag())
        || message.has_tag(RequestKind::Dem.message_tag())
        || message.has_tag(RequestKind::Vector.message_tag())
    {
        let outcome = message.into_transferable::<TileRequestOutcome>()?;
        let data = serde_json::to_vec(&*outcome).map_err(|source| SendError::Transmission {
            operation: "encoding a tile request outcome",
            source: Box::new(source),
        })?;
        return Ok((WebMessageTag::TileRequestOutcome, PayloadBytes::Vec(data)));
    }
    if message.has_tag(ProvidedImagesReport::message_tag()) {
        let report = message.into_transferable::<ProvidedImagesReport>()?;
        let data = serde_json::to_vec(&*report).map_err(|source| SendError::Transmission {
            operation: "encoding a provided images report",
            source: Box::new(source),
        })?;
        return Ok((WebMessageTag::ProvidedImages, PayloadBytes::Vec(data)));
    }
    let tag = message
        .tag()
        .as_any()
        .downcast_ref::<WebMessageTag>()
        .copied()
        .ok_or(SendError::UnsupportedTag { tag: message.tag() })?;
    let transferable = message.into_transferable::<FlatBufferTransferable>()?;
    tracing::debug!(
        ?tag,
        bytes = transferable.data().len(),
        "sending worker result"
    );
    Ok((tag, PayloadBytes::Transferable(transferable)))
}

pub(crate) fn decode_message(
    tag: WebMessageTag,
    buffer: ArrayBuffer,
) -> Result<Message, CallError> {
    let bytes = Uint8Array::new(&buffer).to_vec();
    decode_message_bytes(tag, bytes, 0)
}

pub(crate) fn decode_message_bytes(
    tag: WebMessageTag,
    bytes: Vec<u8>,
    offset: usize,
) -> Result<Message, CallError> {
    let slice = bytes.get(offset..).unwrap_or_default();
    if tag == WebMessageTag::TrackedPayload {
        let header = slice.first_chunk::<12>().ok_or_else(|| {
            CallError::Deserialize(Box::new(TrackedPayloadError::Header { bytes: slice.len() }))
        })?;
        let inner_tag = WebMessageTag::from_u32(u32::from_le_bytes([
            header[0], header[1], header[2], header[3],
        ]))
        .map_err(|error| CallError::Deserialize(Box::new(error)))?;
        if matches!(
            inner_tag,
            WebMessageTag::TrackedPayload | WebMessageTag::BatchPayload
        ) {
            return Err(CallError::Deserialize(Box::new(
                TrackedPayloadError::Nested,
            )));
        }
        let attempt = u64::from_le_bytes([
            header[4], header[5], header[6], header[7], header[8], header[9], header[10],
            header[11],
        ]);
        return decode_message_bytes(inner_tag, bytes, offset + 12)
            .map(|message| message.with_attempt(attempt));
    }
    if tag == WebMessageTag::TileRequestOutcome {
        let outcome: TileRequestOutcome = serde_json::from_slice(slice)
            .map_err(|source| CallError::Deserialize(Box::new(source)))?;
        return Ok(IntoMessage::into(outcome));
    }
    if tag == WebMessageTag::ProvidedImages {
        let report: ProvidedImagesReport = serde_json::from_slice(slice)
            .map_err(|source| CallError::Deserialize(Box::new(source)))?;
        return Ok(IntoMessage::into(report));
    }
    Ok(Message::new(
        tag.to_static(),
        Box::new(FlatBufferTransferable::from_vec(tag, bytes, offset)),
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
    let buffer = encode_call_finished(attempt);
    post_wire_message(WebMessageTag::CallFinished, &buffer)
}

#[derive(Debug, Error)]
pub(crate) enum TrackedPayloadError {
    #[error("tracked worker payload has {bytes} bytes; its header requires 12")]
    Header { bytes: usize },
    #[error("batched worker payload truncated at byte {offset} of {total}")]
    BatchTruncated { offset: u32, total: u32 },
    #[error("tracked worker payload contains a nested tracking envelope")]
    Nested,
    #[error("finished call payload has {bytes} bytes; its attempt requires 8")]
    Finished { bytes: usize },
}
