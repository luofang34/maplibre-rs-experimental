//! Worker reply context for [`super::SchedulerAsyncProcedureCall`].

#[cfg(target_arch = "wasm32")]
use std::sync::Mutex;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::Sender,
    Arc,
};

use super::ArrivalWaker;
use crate::io::apc::{Context, IntoMessage, Message, SendError};

#[cfg(target_arch = "wasm32")]
fn is_intermediate_vector_layer(message: &Message) -> bool {
    use crate::vector::transferables::TileTessellated as _;
    matches!(
        message
            .tag()
            .as_any()
            .downcast_ref::<crate::vector::transferables::VectorMessageTag>(),
        Some(
            crate::vector::transferables::VectorMessageTag::LayerMissing
                | crate::vector::transferables::VectorMessageTag::LayerTessellated
                | crate::vector::transferables::VectorMessageTag::SymbolLayerTessellated
                | crate::vector::transferables::VectorMessageTag::LayerIndexed
        )
    ) || message.has_tag(crate::sdf::provided::ProvidedImagesReport::message_tag())
        || message
            .downcast_ref::<crate::vector::transferables::DefaultTileTessellated>()
            .is_some_and(|tile| !tile.pending_symbols())
}

/// Reply sender whose clones share the scheduler-backed APC's receiving channel.
#[derive(Clone)]
pub struct SchedulerContext {
    pub(crate) sender: Sender<Message>,
    pub(crate) waker: Option<ArrivalWaker>,
    batch_sender: Option<Sender<Vec<Message>>>,
    cancel_flag: Option<Arc<AtomicBool>>,
    sent_any: Option<Arc<AtomicBool>>,
    #[cfg(target_arch = "wasm32")]
    pending: Arc<Mutex<Vec<Message>>>,
}

impl SchedulerContext {
    #[cfg(test)]
    pub(crate) fn new(sender: Sender<Message>, waker: Option<ArrivalWaker>) -> Self {
        Self {
            sender,
            waker,
            batch_sender: None,
            cancel_flag: None,
            sent_any: None,
            #[cfg(target_arch = "wasm32")]
            pending: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub(super) fn for_call(
        sender: Sender<Message>,
        batch_sender: Sender<Vec<Message>>,
        waker: Option<ArrivalWaker>,
        cancel_flag: Arc<AtomicBool>,
        sent_any: Arc<AtomicBool>,
    ) -> Self {
        Self {
            sender,
            waker,
            batch_sender: Some(batch_sender),
            cancel_flag: Some(cancel_flag),
            sent_any: Some(sent_any),
            #[cfg(target_arch = "wasm32")]
            pending: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn is_cancelled(&self) -> bool {
        self.cancel_flag
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Acquire))
    }

    fn clear_pending(&self) {
        #[cfg(target_arch = "wasm32")]
        crate::platform::lock_unpoisoned(&self.pending).clear();
    }

    fn send_batch(&self, messages: Vec<Message>) -> Result<(), SendError> {
        if self.is_cancelled() {
            return Err(SendError::Abandoned);
        }
        if messages.is_empty() {
            return Ok(());
        }
        if let Some(batch_sender) = &self.batch_sender {
            batch_sender
                .send(messages)
                .map_err(|source| SendError::Transmission {
                    operation: "sending a result to the caller",
                    source: Box::new(source),
                })?;
        } else {
            for msg in messages {
                self.sender
                    .send(msg)
                    .map_err(|source| SendError::Transmission {
                        operation: "sending a result to the caller",
                        source: Box::new(source),
                    })?;
            }
        }
        if let Some(sent) = &self.sent_any {
            sent.store(true, Ordering::Release);
        }
        if let Some(wake) = &self.waker {
            wake();
        }
        Ok(())
    }

    #[cfg(target_arch = "wasm32")]
    pub(super) fn flush(&self) -> Result<(), SendError> {
        if self.is_cancelled() {
            self.clear_pending();
            return Err(SendError::Abandoned);
        }
        let buffered = std::mem::take(&mut *crate::platform::lock_unpoisoned(&self.pending));
        self.send_batch(buffered)
    }
}

impl Context for SchedulerContext {
    fn send_back<T: IntoMessage>(&self, message: T) -> Result<(), SendError> {
        if self.is_cancelled() {
            self.clear_pending();
            return Err(SendError::Abandoned);
        }
        let message = message.into();
        #[cfg(target_arch = "wasm32")]
        {
            if is_intermediate_vector_layer(&message) {
                crate::platform::lock_unpoisoned(&self.pending).push(message);
                return Ok(());
            }
            let mut buffered =
                std::mem::take(&mut *crate::platform::lock_unpoisoned(&self.pending));
            buffered.push(message);
            self.send_batch(buffered)
        }
        #[cfg(not(target_arch = "wasm32"))]
        self.send_batch(vec![message])
    }
}
