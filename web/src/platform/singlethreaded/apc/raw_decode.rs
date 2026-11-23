//! Single-copy scratch and direct-slice decoders for queued ST worker wire payloads.

#![deny(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::{
    cell::RefCell,
    rc::{Rc, Weak},
    vec::IntoIter,
};

use js_sys::{ArrayBuffer, Uint8Array};
use maplibre::io::apc::Message;

use super::{
    decode_message, raw_queue::QueuedMessage, wire::decode_message_bytes, ReceivedType,
    WebMessageTag,
};
use crate::platform::singlethreaded::transferables::FlatBufferTransferable;

const MAX_ITEM_SCRATCH_BYTES: usize = 256 * 1024;
const TINY_STACK_DECODE_BYTES: usize = 256;

thread_local! {
    static DECODE_SCRATCH: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Consumes queued wire messages on demand, decoding payloads lazily one item at a time.
pub struct PassingReceiveIter {
    inner: IntoIter<QueuedMessage>,
    received: Option<Weak<ReceivedType>>,
}

impl PassingReceiveIter {
    pub(super) fn new(items: Vec<QueuedMessage>, received: Option<Weak<ReceivedType>>) -> Self {
        Self {
            inner: items.into_iter(),
            received,
        }
    }
}

fn try_decode_from_bytes(
    effective_tag: WebMessageTag,
    attempt: Option<u64>,
    slice: &[u8],
) -> Option<Message> {
    let transferable = match effective_tag {
        WebMessageTag::TileTessellated => {
            FlatBufferTransferable::from_bytes_tile_tessellated(slice)
        }
        WebMessageTag::LayerMissing => FlatBufferTransferable::from_bytes_layer_missing(slice),
        WebMessageTag::LayerTessellated => {
            FlatBufferTransferable::from_bytes_layer_tessellated(slice)
        }
        WebMessageTag::SymbolLayerTessellated => {
            FlatBufferTransferable::from_bytes_symbol_layer(slice)
        }
        WebMessageTag::LayerIndexed => FlatBufferTransferable::from_bytes_layer_indexed(slice),
        WebMessageTag::LayerRaster => FlatBufferTransferable::from_bytes_layer_raster(slice),
        WebMessageTag::LayerRasterMissing => {
            FlatBufferTransferable::from_bytes_layer_raster_missing(slice)
        }
        WebMessageTag::LayerDem => FlatBufferTransferable::from_bytes_layer_dem(slice),
        WebMessageTag::LayerDemMissing => {
            FlatBufferTransferable::from_bytes_layer_dem_missing(slice)
        }
        _ => return None,
    }?;
    let msg = Message::new(effective_tag.to_static(), Box::new(transferable));
    Some(match attempt {
        Some(id) => msg.with_attempt(id),
        None => msg,
    })
}

fn try_decode_direct_js(
    effective_tag: WebMessageTag,
    attempt: Option<u64>,
    buffer: &ArrayBuffer,
    offset: u32,
    len: u32,
) -> Option<Result<Message, ()>> {
    if matches!(
        effective_tag,
        WebMessageTag::TileTessellated
            | WebMessageTag::LayerMissing
            | WebMessageTag::LayerRasterMissing
            | WebMessageTag::LayerDemMissing
    ) && len > 0
        && (len as usize) <= TINY_STACK_DECODE_BYTES
    {
        let end = offset.checked_add(len)?;
        if end <= buffer.byte_length() {
            let mut stack = [0_u8; TINY_STACK_DECODE_BYTES];
            let dst = stack.get_mut(..len as usize)?;
            Uint8Array::new_with_byte_offset_and_length(buffer, offset, len).copy_to(dst);
            return Some(try_decode_from_bytes(effective_tag, attempt, dst).ok_or(()));
        }
    }
    if matches!(
        effective_tag,
        WebMessageTag::LayerTessellated
            | WebMessageTag::SymbolLayerTessellated
            | WebMessageTag::LayerIndexed
    ) && (len as usize) <= MAX_ITEM_SCRATCH_BYTES
        && len > 0
    {
        let end = offset.checked_add(len)?;
        if end <= buffer.byte_length() {
            let decoded = DECODE_SCRATCH.with(|cell| {
                let mut scratch = cell.borrow_mut();
                scratch.resize(len as usize, 0);
                let res = scratch.get_mut(..len as usize).and_then(|dst| {
                    Uint8Array::new_with_byte_offset_and_length(buffer, offset, len).copy_to(dst);
                    try_decode_from_bytes(effective_tag, attempt, dst)
                });
                if scratch.capacity() > MAX_ITEM_SCRATCH_BYTES {
                    scratch.shrink_to(MAX_ITEM_SCRATCH_BYTES);
                }
                res
            });
            return Some(decoded.ok_or(()));
        }
    }
    let decoded = match effective_tag {
        WebMessageTag::LayerTessellated => {
            FlatBufferTransferable::from_js_layer_tessellated(buffer, offset, len)
        }
        WebMessageTag::SymbolLayerTessellated => {
            FlatBufferTransferable::from_js_symbol_layer(buffer, offset, len)
        }
        WebMessageTag::LayerIndexed => {
            FlatBufferTransferable::from_js_layer_indexed(buffer, offset, len)
        }
        WebMessageTag::LayerRaster => {
            FlatBufferTransferable::from_js_layer_raster(buffer, offset, len)
        }
        WebMessageTag::LayerDem => FlatBufferTransferable::from_js_layer_dem(buffer, offset, len),
        _ => return None,
    };
    let Some(transferable) = decoded else {
        return Some(Err(()));
    };
    let msg = Message::new(effective_tag.to_static(), Box::new(transferable));
    Some(Ok(match attempt {
        Some(id) => msg.with_attempt(id),
        None => msg,
    }))
}

fn decode_raw_item(
    wire_tag: WebMessageTag,
    effective_tag: WebMessageTag,
    attempt: Option<u64>,
    buffer: ArrayBuffer,
) -> Option<Message> {
    let offset = if wire_tag == WebMessageTag::TrackedPayload {
        12_u32
    } else {
        0_u32
    };
    let len = buffer.byte_length().saturating_sub(offset);
    if let Some(result) = try_decode_direct_js(effective_tag, attempt, &buffer, offset, len) {
        return match result {
            Ok(msg) => Some(msg),
            Err(()) => {
                tracing::warn!(
                    ?wire_tag,
                    ?effective_tag,
                    "dropping malformed worker payload"
                );
                None
            }
        };
    }
    match decode_message(wire_tag, buffer) {
        Ok(message) => Some(message),
        Err(error) => {
            tracing::warn!(?wire_tag, %error, "dropping malformed worker payload");
            None
        }
    }
}

fn decode_raw_slice_item(
    effective_tag: WebMessageTag,
    attempt: Option<u64>,
    buffer: &ArrayBuffer,
    byte_offset: u32,
    byte_len: u32,
) -> Option<Message> {
    if let Some(result) =
        try_decode_direct_js(effective_tag, attempt, buffer, byte_offset, byte_len)
    {
        return match result {
            Ok(msg) => Some(msg),
            Err(()) => {
                tracing::warn!(?effective_tag, "dropping malformed batched worker payload");
                None
            }
        };
    }
    let bytes = Uint8Array::new_with_byte_offset_and_length(buffer, byte_offset, byte_len).to_vec();
    match decode_message_bytes(effective_tag, bytes, 0) {
        Ok(message) => Some(match attempt {
            Some(id) => message.with_attempt(id),
            None => message,
        }),
        Err(error) => {
            tracing::warn!(
                ?effective_tag,
                %error,
                "dropping malformed batched worker payload"
            );
            None
        }
    }
}

fn decode_owned_slice_item(
    effective_tag: WebMessageTag,
    attempt: Option<u64>,
    data: &Rc<Vec<u8>>,
    byte_offset: u32,
    byte_len: u32,
) -> Option<Message> {
    let start = byte_offset as usize;
    let end = start.checked_add(byte_len as usize)?;
    let slice = data.get(start..end)?;
    if matches!(
        effective_tag,
        WebMessageTag::TileTessellated
            | WebMessageTag::LayerMissing
            | WebMessageTag::LayerTessellated
            | WebMessageTag::SymbolLayerTessellated
            | WebMessageTag::LayerIndexed
            | WebMessageTag::LayerRaster
            | WebMessageTag::LayerRasterMissing
            | WebMessageTag::LayerDem
            | WebMessageTag::LayerDemMissing
    ) {
        if let Some(msg) = try_decode_from_bytes(effective_tag, attempt, slice) {
            return Some(msg);
        }
        tracing::warn!(?effective_tag, "dropping malformed batched worker payload");
        return None;
    }
    match decode_message_bytes(effective_tag, slice.to_vec(), 0) {
        Ok(message) => Some(match attempt {
            Some(id) => message.with_attempt(id),
            None => message,
        }),
        Err(error) => {
            tracing::warn!(
                ?effective_tag,
                %error,
                "dropping malformed batched worker payload"
            );
            None
        }
    }
}

impl Iterator for PassingReceiveIter {
    type Item = Message;

    fn next(&mut self) -> Option<Self::Item> {
        for item in self.inner.by_ref() {
            let cancelled = item.attempt().is_some_and(|attempt| {
                self.received
                    .as_ref()
                    .and_then(Weak::upgrade)
                    .is_some_and(|cell| {
                        cell.try_borrow()
                            .is_ok_and(|guard| guard.cancelled.contains(&attempt))
                    })
            });
            if cancelled {
                continue;
            }
            let decoded = match item {
                QueuedMessage::Decoded(message) => Some(message),
                QueuedMessage::Raw {
                    wire_tag,
                    effective_tag,
                    attempt,
                    buffer,
                    ..
                } => decode_raw_item(wire_tag, effective_tag, attempt, buffer),
                QueuedMessage::RawSlice {
                    effective_tag,
                    attempt,
                    buffer,
                    byte_offset,
                    byte_len,
                } => decode_raw_slice_item(effective_tag, attempt, &buffer, byte_offset, byte_len),
                QueuedMessage::OwnedSlice {
                    effective_tag,
                    attempt,
                    data,
                    byte_offset,
                    byte_len,
                } => decode_owned_slice_item(effective_tag, attempt, &data, byte_offset, byte_len),
            };
            if let Some(msg) = decoded {
                return Some(msg);
            }
        }
        None
    }
}
