//! Delivers incoming single-threaded worker messages into the main-thread APC queue.

use maplibre::io::apc::CallError;

use super::{decode_call_finished, QueuedMessage, ReceivedType, WebMessageTag};
use crate::error::{JSError, WebError};

/// Delivers a `[tag, ArrayBuffer]` message to `received` without leaking raw `Rc` pointers.
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
            let attempt = decode_call_finished(&buffer)?;
            received.cancelled.remove(&attempt);
            received.finished.push(attempt);
            if !received.messages.is_empty() || !received.queued.is_empty() {
                received.waker.clone()
            } else {
                None
            }
        } else {
            let received_ref = &mut *received;
            QueuedMessage::push_from_wire(
                &mut received_ref.queued,
                tag,
                buffer,
                &received_ref.cancelled,
            )?;
            if !received_ref.messages.is_empty() || !received_ref.queued.is_empty() {
                received_ref.waker.clone()
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
