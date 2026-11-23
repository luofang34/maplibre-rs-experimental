//! Deferred JS `ArrayBuffer` queue so worker payloads stay outside WASM linear memory until
//! consumed one message at a time, with single-copy batch unpack for compact tile batches.

#![deny(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::rc::Rc;

use js_sys::{ArrayBuffer, Uint8Array};
use maplibre::io::apc::{CallError, Message};

use super::{
    decode_message,
    wire::{decode_outcome_slice, BATCH_ITEM_HEADER_BYTES},
    TrackedPayloadError, WebMessageTag,
};

const MAX_BATCH_COPY_BYTES: u32 = 512 * 1024;

pub(crate) enum QueuedMessage {
    Decoded(Message),
    Raw {
        wire_tag: WebMessageTag,
        effective_tag: WebMessageTag,
        attempt: Option<u64>,
        byte_len: u32,
        buffer: ArrayBuffer,
    },
    RawSlice {
        effective_tag: WebMessageTag,
        attempt: Option<u64>,
        buffer: ArrayBuffer,
        byte_offset: u32,
        byte_len: u32,
    },
    OwnedSlice {
        effective_tag: WebMessageTag,
        attempt: Option<u64>,
        data: Rc<Vec<u8>>,
        byte_offset: u32,
        byte_len: u32,
    },
}

fn parse_batch_header(
    bytes: &[u8],
    total: u32,
) -> Result<(WebMessageTag, Option<u64>, u32), CallError> {
    let hdr = bytes
        .first_chunk::<BATCH_ITEM_HEADER_BYTES>()
        .ok_or_else(|| {
            CallError::Deserialize(Box::new(TrackedPayloadError::BatchTruncated {
                offset: 0,
                total,
            }))
        })?;
    let inner_tag = WebMessageTag::from_u32(u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]))
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
    let payload_len = u32::from_le_bytes([hdr[16], hdr[17], hdr[18], hdr[19]]);
    Ok((inner_tag, has_attempt.then_some(raw_attempt), payload_len))
}

impl QueuedMessage {
    fn decode_batch_item_js(
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
        let (inner_tag, attempt, payload_len) = parse_batch_header(&hdr, total)?;
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
            let msg = decode_outcome_slice(inner_tag, &bytes)?;
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

    fn decode_batch_item_owned(
        shared: &Rc<Vec<u8>>,
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
        let cur = *cursor as usize;
        let hdr = shared
            .get(cur..cur + BATCH_ITEM_HEADER_BYTES)
            .unwrap_or_default();
        let (inner_tag, attempt, payload_len) = parse_batch_header(hdr, total)?;
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
            let start = payload_offset as usize;
            let slice = shared
                .get(start..start + payload_len as usize)
                .unwrap_or_default();
            let msg = decode_outcome_slice(inner_tag, slice)?;
            let msg = match attempt {
                Some(id) => msg.with_attempt(id),
                None => msg,
            };
            return Ok(Self::Decoded(msg));
        }
        Ok(Self::OwnedSlice {
            effective_tag: inner_tag,
            attempt,
            data: Rc::clone(shared),
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
        if total <= MAX_BATCH_COPY_BYTES {
            let mut raw = vec![0_u8; total as usize];
            if total > 0 {
                Uint8Array::new(&buffer).copy_to(&mut raw);
            }
            let shared = Rc::new(raw);
            while cursor < total {
                out.push(Self::decode_batch_item_owned(&shared, &mut cursor, total)?);
            }
            return Ok(());
        }
        while cursor < total {
            out.push(Self::decode_batch_item_js(&buffer, &mut cursor, total)?);
        }
        Ok(())
    }

    pub(crate) fn from_wire(
        wire_tag: WebMessageTag,
        buffer: ArrayBuffer,
    ) -> Result<Self, CallError> {
        let byte_len = buffer.byte_length();
        let (effective_tag, attempt) = if wire_tag == WebMessageTag::TrackedPayload {
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
            byte_len,
            buffer,
        })
    }

    pub(super) fn raw_byte_len(&self) -> usize {
        match self {
            Self::Decoded(_) => 0,
            Self::Raw { byte_len, .. }
            | Self::RawSlice { byte_len, .. }
            | Self::OwnedSlice { byte_len, .. } => *byte_len as usize,
        }
    }

    pub(super) fn attempt(&self) -> Option<u64> {
        match self {
            Self::Decoded(message) => message.attempt(),
            Self::Raw { attempt, .. }
            | Self::RawSlice { attempt, .. }
            | Self::OwnedSlice { attempt, .. } => *attempt,
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
            }
            | Self::OwnedSlice {
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
