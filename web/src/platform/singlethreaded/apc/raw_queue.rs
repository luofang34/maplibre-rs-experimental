//! Deferred JS `ArrayBuffer` queue so worker payloads stay outside WASM linear memory until
//! consumed one message at a time.

#![deny(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::vec::IntoIter;

use js_sys::{ArrayBuffer, Uint8Array};
use maplibre::io::apc::{CallError, Message};

use super::{
    decode_message,
    wire::{decode_message_bytes, BATCH_ITEM_HEADER_BYTES},
    TrackedPayloadError, WebMessageTag,
};
use crate::platform::singlethreaded::transferables::FlatBufferTransferable;

pub(crate) enum QueuedMessage {
    Decoded(Message),
    Raw {
        wire_tag: WebMessageTag,
        effective_tag: WebMessageTag,
        attempt: Option<u64>,
        buffer: ArrayBuffer,
    },
    RawSlice {
        effective_tag: WebMessageTag,
        attempt: Option<u64>,
        buffer: ArrayBuffer,
        byte_offset: u32,
        byte_len: u32,
    },
}

impl QueuedMessage {
    fn decode_batch_item(
        buffer: &ArrayBuffer,
        cursor: &mut u32,
        total: u32,
    ) -> Result<Self, CallError> {
        let hdr_len = BATCH_ITEM_HEADER_BYTES as u32;
        if total - *cursor < hdr_len {
            return Err(CallError::Deserialize(Box::new(
                TrackedPayloadError::BatchTruncated {
                    offset: *cursor,
                    total,
                },
            )));
        }
        let mut hdr = [0_u8; BATCH_ITEM_HEADER_BYTES];
        Uint8Array::new_with_byte_offset_and_length(buffer, *cursor, hdr_len).copy_to(&mut hdr);
        let inner_tag =
            WebMessageTag::from_u32(u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]))
                .map_err(|error| CallError::Deserialize(Box::new(error)))?;
        if matches!(
            inner_tag,
            WebMessageTag::TrackedPayload | WebMessageTag::BatchPayload
        ) {
            return Err(CallError::Deserialize(Box::new(
                TrackedPayloadError::Nested,
            )));
        }
        let has_attempt = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) != 0;
        let raw_attempt = u64::from_le_bytes([
            hdr[8], hdr[9], hdr[10], hdr[11], hdr[12], hdr[13], hdr[14], hdr[15],
        ]);
        let attempt = has_attempt.then_some(raw_attempt);
        let payload_len = u32::from_le_bytes([hdr[16], hdr[17], hdr[18], hdr[19]]);
        let payload_offset = *cursor + hdr_len;
        if total - payload_offset < payload_len {
            return Err(CallError::Deserialize(Box::new(
                TrackedPayloadError::BatchTruncated {
                    offset: payload_offset,
                    total,
                },
            )));
        }
        *cursor = payload_offset + payload_len;
        if matches!(
            inner_tag,
            WebMessageTag::TileRequestOutcome | WebMessageTag::ProvidedImages
        ) {
            let bytes =
                Uint8Array::new_with_byte_offset_and_length(buffer, payload_offset, payload_len)
                    .to_vec();
            let msg = decode_message_bytes(inner_tag, bytes, 0)?;
            let msg = match attempt {
                Some(id) => msg.with_attempt(id),
                None => msg,
            };
            return Ok(Self::Decoded(msg));
        }
        Ok(Self::RawSlice {
            effective_tag: inner_tag,
            attempt,
            buffer: buffer.clone(),
            byte_offset: payload_offset,
            byte_len: payload_len,
        })
    }

    pub(crate) fn push_from_wire(
        out: &mut Vec<Self>,
        wire_tag: WebMessageTag,
        buffer: ArrayBuffer,
    ) -> Result<(), CallError> {
        if wire_tag != WebMessageTag::BatchPayload {
            out.push(Self::from_wire(wire_tag, buffer)?);
            return Ok(());
        }
        let total = buffer.byte_length();
        let mut cursor = 0_u32;
        while cursor < total {
            out.push(Self::decode_batch_item(&buffer, &mut cursor, total)?);
        }
        Ok(())
    }

    pub(crate) fn from_wire(
        wire_tag: WebMessageTag,
        buffer: ArrayBuffer,
    ) -> Result<Self, CallError> {
        let (effective_tag, attempt) = if wire_tag == WebMessageTag::TrackedPayload {
            let byte_len = buffer.byte_length();
            if byte_len < 12 {
                return Err(CallError::Deserialize(Box::new(
                    TrackedPayloadError::Header {
                        bytes: byte_len as usize,
                    },
                )));
            }
            let mut header = [0_u8; 12];
            Uint8Array::new_with_byte_offset_and_length(&buffer, 0, 12).copy_to(&mut header);
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
            (inner_tag, Some(attempt))
        } else {
            (wire_tag, None)
        };

        if matches!(
            effective_tag,
            WebMessageTag::TileRequestOutcome | WebMessageTag::ProvidedImages
        ) {
            return decode_message(wire_tag, buffer).map(Self::Decoded);
        }

        Ok(Self::Raw {
            wire_tag,
            effective_tag,
            attempt,
            buffer,
        })
    }

    pub(super) fn raw_byte_len(&self) -> usize {
        match self {
            Self::Decoded(_) => 0,
            Self::Raw { buffer, .. } => buffer.byte_length() as usize,
            Self::RawSlice { byte_len, .. } => *byte_len as usize,
        }
    }

    pub(super) fn attempt(&self) -> Option<u64> {
        match self {
            Self::Decoded(message) => message.attempt(),
            Self::Raw { attempt, .. } | Self::RawSlice { attempt, .. } => *attempt,
        }
    }

    pub(super) fn matches<F: FnMut(&Message) -> bool>(&self, filter: &mut F) -> bool {
        match self {
            Self::Decoded(message) => filter(message),
            Self::Raw {
                effective_tag,
                attempt,
                ..
            }
            | Self::RawSlice {
                effective_tag,
                attempt,
                ..
            } => {
                let probe = match attempt {
                    Some(attempt) => {
                        Message::new(effective_tag.to_static(), Box::new(())).with_attempt(*attempt)
                    }
                    None => Message::new(effective_tag.to_static(), Box::new(())),
                };
                filter(&probe)
            }
        }
    }
}

pub struct PassingReceiveIter {
    inner: IntoIter<QueuedMessage>,
}

impl PassingReceiveIter {
    pub(super) fn new(items: Vec<QueuedMessage>) -> Self {
        Self {
            inner: items.into_iter(),
        }
    }
}

fn try_decode_direct_js(
    effective_tag: WebMessageTag,
    attempt: Option<u64>,
    buffer: &ArrayBuffer,
    offset: u32,
    len: u32,
) -> Option<Result<Message, ()>> {
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

impl Iterator for PassingReceiveIter {
    type Item = Message;

    fn next(&mut self) -> Option<Self::Item> {
        for item in self.inner.by_ref() {
            let decoded = match item {
                QueuedMessage::Decoded(message) => Some(message),
                QueuedMessage::Raw {
                    wire_tag,
                    effective_tag,
                    attempt,
                    buffer,
                } => decode_raw_item(wire_tag, effective_tag, attempt, buffer),
                QueuedMessage::RawSlice {
                    effective_tag,
                    attempt,
                    buffer,
                    byte_offset,
                    byte_len,
                } => decode_raw_slice_item(effective_tag, attempt, &buffer, byte_offset, byte_len),
            };
            if let Some(msg) = decoded {
                return Some(msg);
            }
        }
        None
    }
}
