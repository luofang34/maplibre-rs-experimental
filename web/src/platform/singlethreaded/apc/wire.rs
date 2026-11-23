//! Wire encoding, per-tile message coalescing, and decoding for single-threaded web workers.

#![deny(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::{
    cell::RefCell,
    sync::{Arc, Mutex, MutexGuard},
};

use js_sys::{ArrayBuffer, Uint8Array};
#[cfg(any(test, feature = "renderer"))]
use maplibre::coords::{WorldTileCoords, ZoomLevel};
#[cfg(any(test, feature = "renderer"))]
use maplibre::io::apc::CallError;
use maplibre::{
    io::{
        apc::{Context, IntoMessage, Message, SendError},
        tile_retry::{RequestDisposition, RequestKind, TileRequestOutcome},
    },
    sdf::provided::{ProvidedImagesReport, ProvidedImagesState},
};
#[cfg(any(test, feature = "renderer"))]
use thiserror::Error;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{DedicatedWorkerGlobalScope, MessagePort};

use super::WebMessageTag;
use crate::{error::WebError, platform::singlethreaded::transferables::FlatBufferTransferable};

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

#[derive(Clone, Default)]
pub struct PassingContext {
    pending: Arc<Mutex<Vec<BufferedItem>>>,
}

impl PassingContext {
    pub fn new() -> Self {
        Self::default()
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
        if tag.is_intermediate_layer()
            || tag == WebMessageTag::ProvidedImages
            || (tag == WebMessageTag::TileTessellated
                && payload.as_slice().get(9).is_none_or(|&b| b == 0))
        {
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
        return Ok((
            WebMessageTag::TileRequestOutcome,
            PayloadBytes::Vec(encode_outcome(&outcome)),
        ));
    }
    if message.has_tag(ProvidedImagesReport::message_tag()) {
        let report = message.into_transferable::<ProvidedImagesReport>()?;
        return Ok((
            WebMessageTag::ProvidedImages,
            PayloadBytes::Vec(encode_provided_report(&report)),
        ));
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

#[cfg(any(test, feature = "renderer"))]
pub(crate) fn decode_message(
    tag: WebMessageTag,
    buffer: ArrayBuffer,
) -> Result<Message, CallError> {
    let bytes = Uint8Array::new(&buffer).to_vec();
    decode_message_bytes(tag, bytes, 0)
}

#[cfg(any(test, feature = "renderer"))]
pub(crate) fn decode_outcome_slice(tag: WebMessageTag, slice: &[u8]) -> Result<Message, CallError> {
    if tag == WebMessageTag::TileRequestOutcome {
        let outcome = decode_outcome(slice)
            .ok_or_else(|| CallError::Deserialize(Box::new(TrackedPayloadError::WirePayload)))?;
        return Ok(IntoMessage::into(outcome));
    }
    if tag == WebMessageTag::ProvidedImages {
        let report = decode_provided_report(slice)
            .ok_or_else(|| CallError::Deserialize(Box::new(TrackedPayloadError::WirePayload)))?;
        return Ok(IntoMessage::into(report));
    }
    Err(CallError::Deserialize(Box::new(
        TrackedPayloadError::WirePayload,
    )))
}

#[cfg(any(test, feature = "renderer"))]
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
    if matches!(
        tag,
        WebMessageTag::TileRequestOutcome | WebMessageTag::ProvidedImages
    ) {
        return decode_outcome_slice(tag, slice);
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
#[cfg(any(test, feature = "renderer"))]
pub(crate) fn decode_call_finished(buffer: &ArrayBuffer) -> Result<u64, CallError> {
    let len = buffer.byte_length() as usize;
    if len != 8 {
        return Err(CallError::Deserialize(Box::new(
            TrackedPayloadError::Finished { bytes: len },
        )));
    }
    let mut bytes = [0_u8; 8];
    Uint8Array::new(buffer).copy_to(&mut bytes);
    Ok(u64::from_le_bytes(bytes))
}

/// Tells the main thread this worker stopped running the call for `attempt`.
pub(crate) fn post_call_finished(attempt: u64) -> Result<(), SendError> {
    let buffer = encode_call_finished(attempt);
    post_wire_message(WebMessageTag::CallFinished, &buffer)
}

#[cfg(any(test, feature = "renderer"))]
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
    #[error("worker status payload could not be decoded")]
    WirePayload,
}

fn encode_outcome(outcome: &TileRequestOutcome) -> Vec<u8> {
    let mut out = Vec::with_capacity(20);
    out.extend_from_slice(&outcome.coords.x.to_le_bytes());
    out.extend_from_slice(&outcome.coords.y.to_le_bytes());
    out.push(u8::from(outcome.coords.z));
    out.push(match outcome.kind {
        RequestKind::Raster => 0,
        RequestKind::Dem => 1,
        RequestKind::Vector => 2,
    });
    out.push(match outcome.disposition {
        RequestDisposition::Complete => 0,
        RequestDisposition::Retry => 1,
    });
    out.push(u8::from(outcome.attempt.is_some()));
    out.extend_from_slice(&outcome.attempt.unwrap_or(0).to_le_bytes());
    out
}

#[cfg(any(test, feature = "renderer"))]
fn decode_outcome(slice: &[u8]) -> Option<TileRequestOutcome> {
    let bytes = <[u8; 20]>::try_from(slice).ok()?;
    let coords = WorldTileCoords {
        x: i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        y: i32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
        z: ZoomLevel::new(bytes[8]),
    };
    let kind = match bytes[9] {
        0 => RequestKind::Raster,
        1 => RequestKind::Dem,
        2 => RequestKind::Vector,
        _ => return None,
    };
    let disposition = match bytes[10] {
        0 => RequestDisposition::Complete,
        1 => RequestDisposition::Retry,
        _ => return None,
    };
    let attempt = match bytes[11] {
        0 => None,
        1 => Some(u64::from_le_bytes([
            bytes[12], bytes[13], bytes[14], bytes[15], bytes[16], bytes[17], bytes[18], bytes[19],
        ])),
        _ => return None,
    };
    Some(TileRequestOutcome {
        coords,
        kind,
        attempt,
        disposition,
    })
}

fn encode_provided_report(report: &ProvidedImagesReport) -> Vec<u8> {
    let extra: usize = report.names.iter().map(|s| 4 + s.len()).sum();
    let mut out = Vec::with_capacity(27 + extra);
    out.extend_from_slice(&report.coords.x.to_le_bytes());
    out.extend_from_slice(&report.coords.y.to_le_bytes());
    out.push(u8::from(report.coords.z));
    out.push(match report.state {
        ProvidedImagesState::Awaiting => 0,
        ProvidedImagesState::Settled => 1,
        ProvidedImagesState::Retry => 2,
    });
    out.push(u8::from(report.attempt.is_some()));
    out.extend_from_slice(&report.attempt.unwrap_or(0).to_le_bytes());
    out.extend_from_slice(&report.pixel_ratio.to_le_bytes());
    out.extend_from_slice(&(report.names.len() as u32).to_le_bytes());
    for name in &report.names {
        out.extend_from_slice(&(name.len() as u32).to_le_bytes());
        out.extend_from_slice(name.as_bytes());
    }
    out
}

#[cfg(any(test, feature = "renderer"))]
fn decode_provided_report(slice: &[u8]) -> Option<ProvidedImagesReport> {
    let (hdr, mut rest) = slice.split_first_chunk::<27>()?;
    let coords = WorldTileCoords {
        x: i32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]),
        y: i32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]),
        z: ZoomLevel::new(hdr[8]),
    };
    let state = match hdr[9] {
        0 => ProvidedImagesState::Awaiting,
        1 => ProvidedImagesState::Settled,
        2 => ProvidedImagesState::Retry,
        _ => return None,
    };
    let attempt = match hdr[10] {
        0 => None,
        1 => Some(u64::from_le_bytes([
            hdr[11], hdr[12], hdr[13], hdr[14], hdr[15], hdr[16], hdr[17], hdr[18],
        ])),
        _ => return None,
    };
    let pixel_ratio = f32::from_le_bytes([hdr[19], hdr[20], hdr[21], hdr[22]]);
    let count = u32::from_le_bytes([hdr[23], hdr[24], hdr[25], hdr[26]]) as usize;
    let mut names = Vec::with_capacity(count.min(256));
    for _ in 0..count {
        let (len_bytes, after_len) = rest.split_first_chunk::<4>()?;
        let len = u32::from_le_bytes(*len_bytes) as usize;
        let (name_bytes, after_name) = after_len.split_at_checked(len)?;
        let name = std::str::from_utf8(name_bytes).ok()?;
        names.push(name.to_owned());
        rest = after_name;
    }
    rest.is_empty().then_some(ProvidedImagesReport {
        coords,
        attempt,
        names,
        pixel_ratio,
        state,
    })
}
