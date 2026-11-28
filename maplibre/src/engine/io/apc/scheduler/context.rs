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
        load_optional_flag(self.cancel_flag.as_ref())
    }

    fn clear_pending(&self) {
        #[cfg(target_arch = "wasm32")]
        crate::util::sync::lock_unpoisoned(&self.pending).clear();
    }

    fn send_batch(&self, messages: Vec<Message>) -> Result<(), SendError> {
        if self.is_cancelled() {
            return Err(SendError::Abandoned);
        }
        if messages.is_empty() {
            return Ok(());
        }
        if let Some(batch_sender) = &self.batch_sender {
            send_on_channel(batch_sender, messages).map_err(|source| SendError::Transmission {
                operation: "sending a result to the caller",
                source,
            })?;
        } else {
            for msg in messages {
                send_on_channel(&self.sender, msg).map_err(|source| SendError::Transmission {
                    operation: "sending a result to the caller",
                    source,
                })?;
            }
        }
        mark_optional_flag(self.sent_any.as_ref());
        invoke_optional_waker(self.waker.as_ref());
        Ok(())
    }

    #[cfg(target_arch = "wasm32")]
    pub(super) fn flush(&self) -> Result<(), SendError> {
        if self.is_cancelled() {
            self.clear_pending();
            return Err(SendError::Abandoned);
        }
        let buffered = std::mem::take(&mut *crate::util::sync::lock_unpoisoned(&self.pending));
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
            if message.is_intermediate() {
                crate::util::sync::lock_unpoisoned(&self.pending).push(message);
                return Ok(());
            }
            let mut buffered =
                std::mem::take(&mut *crate::util::sync::lock_unpoisoned(&self.pending));
            buffered.push(message);
            self.send_batch(buffered)
        }
        #[cfg(not(target_arch = "wasm32"))]
        self.send_batch(vec![message])
    }
}

fn load_optional_flag(flag: Option<&Arc<AtomicBool>>) -> bool {
    flag.is_some_and(|flag| flag.load(Ordering::Acquire))
}

fn mark_optional_flag(flag: Option<&Arc<AtomicBool>>) {
    if let Some(flag) = flag {
        flag.store(true, Ordering::Release);
    }
}

fn invoke_optional_waker(waker: Option<&Arc<dyn Fn() + Send + Sync>>) {
    if let Some(wake) = waker {
        wake();
    }
}

fn send_on_channel<T: Send + 'static>(
    sender: &Sender<T>,
    payload: T,
) -> Result<(), Box<dyn std::error::Error + Send>> {
    sender.send(payload).map_err(|source| Box::new(source) as _)
}
