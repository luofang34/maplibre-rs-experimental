//! Scheduler-backed [`AsyncProcedureCall`] and [`SchedulerContext`] implementation.

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
    marker::PhantomData,
    sync::{
        mpsc::{self, Receiver, Sender},
        Arc, Mutex,
    },
    vec::IntoIter,
};

use futures::future::{AbortHandle, Abortable, Aborted};

use super::{
    AsyncProcedure, AsyncProcedureCall, CallError, Context, Input, IntoMessage, Message,
    ProcedureError, SendError,
};
use crate::{
    environment::{OffscreenKernel, OffscreenKernelConfig},
    io::scheduler::Scheduler,
};

#[cfg(target_arch = "wasm32")]
const MAX_ADMITTED_ATTEMPTS_PER_BATCH: usize = 6;

#[cfg(target_arch = "wasm32")]
fn is_intermediate_vector_layer(message: &Message) -> bool {
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
    )
}

/// Reply sender whose clones share the scheduler-backed APC's receiving channel.
#[derive(Clone)]
pub struct SchedulerContext {
    pub(crate) sender: Sender<Message>,
    pub(crate) waker: Option<Arc<dyn Fn() + Send + Sync>>,
    #[cfg(target_arch = "wasm32")]
    pending: Arc<Mutex<Vec<Message>>>,
}

impl SchedulerContext {
    pub(crate) fn new(sender: Sender<Message>, waker: Option<Arc<dyn Fn() + Send + Sync>>) -> Self {
        Self {
            sender,
            waker,
            #[cfg(target_arch = "wasm32")]
            pending: Arc::new(Mutex::new(Vec::new())),
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub(super) fn flush(&self) -> Result<(), SendError> {
        let buffered = std::mem::take(&mut *crate::platform::lock_unpoisoned(&self.pending));
        if buffered.is_empty() {
            return Ok(());
        }
        for msg in buffered {
            self.sender
                .send(msg)
                .map_err(|source| SendError::Transmission {
                    operation: "sending a result to the caller",
                    source: Box::new(source),
                })?;
        }
        if let Some(wake) = &self.waker {
            wake();
        }
        Ok(())
    }
}

impl Context for SchedulerContext {
    fn send_back<T: IntoMessage>(&self, message: T) -> Result<(), SendError> {
        let message = message.into();
        #[cfg(target_arch = "wasm32")]
        {
            if is_intermediate_vector_layer(&message) {
                crate::platform::lock_unpoisoned(&self.pending).push(message);
                return Ok(());
            }
            let buffered = std::mem::take(&mut *crate::platform::lock_unpoisoned(&self.pending));
            for pending_msg in buffered {
                self.sender
                    .send(pending_msg)
                    .map_err(|source| SendError::Transmission {
                        operation: "sending a result to the caller",
                        source: Box::new(source),
                    })?;
            }
        }
        self.sender
            .send(message)
            .map_err(|source| SendError::Transmission {
                operation: "sending a result to the caller",
                source: Box::new(source),
            })?;
        if let Some(wake) = &self.waker {
            wake();
        }
        Ok(())
    }
}

/// An APC using a scheduler for execution and an in-process channel for replies.
///
/// Receiving drains available matches in delivery order. Procedure failures are logged;
/// only preparation and admission failures are returned by [`AsyncProcedureCall::call`].
pub struct SchedulerAsyncProcedureCall<K: OffscreenKernel, S: Scheduler> {
    pub(crate) channel: (Sender<Message>, Receiver<Message>),
    /// Admitted messages retained for sibling consumers in the current frame.
    buffer: RefCell<Vec<Message>>,
    /// Unadmitted arrivals peeked from `channel.1` or deferred to a subsequent frame.
    queued: RefCell<VecDeque<Message>>,
    /// Whether `receive` may admit new messages from `channel` during the current frame.
    batch_open: Cell<bool>,
    scheduler: S,
    phantom_k: PhantomData<K>,
    offscreen_kernel_config: OffscreenKernelConfig,
    /// Tracked calls still running, by attempt, so a cancelled one can be stopped.
    running: Arc<Mutex<HashMap<u64, AbortHandle>>>,
    pub(crate) waker: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl<K: OffscreenKernel, S: Scheduler> SchedulerAsyncProcedureCall<K, S> {
    /// Creates an empty reply channel and keeps the worker kernel configuration for future calls.
    pub fn new(scheduler: S, offscreen_kernel_config: OffscreenKernelConfig) -> Self {
        Self {
            channel: mpsc::channel(),
            buffer: RefCell::new(Vec::new()),
            queued: RefCell::new(VecDeque::new()),
            batch_open: Cell::new(true),
            phantom_k: PhantomData,
            scheduler,
            offscreen_kernel_config,
            running: Arc::default(),
            waker: Mutex::new(None),
        }
    }

    pub(super) fn running(&self) -> std::sync::MutexGuard<'_, HashMap<u64, AbortHandle>> {
        crate::platform::lock_unpoisoned(&self.running)
    }
}

impl<K: OffscreenKernel, S: Scheduler> AsyncProcedureCall<K> for SchedulerAsyncProcedureCall<K, S> {
    type Context = SchedulerContext;
    type ReceiveIterator<F: FnMut(&Message) -> bool> = IntoIter<Message>;

    fn receive<F: FnMut(&Message) -> bool>(&self, mut filter: F) -> Self::ReceiveIterator<F> {
        let mut buffer = self.buffer.borrow_mut();
        // Partial and final tile completions must retain the worker's delivery order.
        let mut ret: Vec<_> = buffer.extract_if(.., |message| filter(message)).collect();

        if !self.batch_open.get() {
            return ret.into_iter();
        }

        #[cfg(target_arch = "wasm32")]
        {
            let mut queued = self.queued.borrow_mut();
            while let Ok(message) = self.channel.1.try_recv() {
                queued.push_back(message);
            }
            let mut admitted_attempts =
                std::collections::HashSet::with_capacity(MAX_ADMITTED_ATTEMPTS_PER_BATCH);
            for existing in buffer.iter().chain(ret.iter()) {
                if let Some(id) = existing.attempt() {
                    admitted_attempts.insert(id);
                }
            }
            let mut deferred = VecDeque::new();
            while let Some(message) = queued.pop_front() {
                let attempt = message.attempt();
                let admit = attempt.is_none_or(|id| {
                    admitted_attempts.contains(&id)
                        || admitted_attempts.len() < MAX_ADMITTED_ATTEMPTS_PER_BATCH
                });
                if !admit {
                    deferred.push_back(message);
                    continue;
                }
                if let Some(id) = attempt {
                    admitted_attempts.insert(id);
                }
                if filter(&message) {
                    ret.push(message);
                } else {
                    buffer.push(message);
                }
            }
            if !deferred.is_empty() {
                self.batch_open.set(false);
            }
            *queued = deferred;
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut queued = self.queued.borrow_mut();
            while let Some(message) = queued.pop_front() {
                if filter(&message) {
                    ret.push(message);
                } else {
                    buffer.push(message);
                }
            }
            while let Ok(message) = self.channel.1.try_recv() {
                tracing::debug!("Data reached main thread: {message:?}");
                if filter(&message) {
                    ret.push(message);
                } else {
                    buffer.push(message);
                }
            }
        }

        ret.into_iter()
    }

    fn call(
        &self,
        input: Input,
        procedure: AsyncProcedure<K, Self::Context>,
    ) -> Result<(), CallError> {
        let sender = self.channel.0.clone();
        let waker = crate::platform::lock_unpoisoned(&self.waker).clone();
        let offscreen_kernel_config = self.offscreen_kernel_config.clone();
        let attempt = input.attempt;
        let (abort, registration) = AbortHandle::new_pair();
        self.running().insert(attempt, abort);
        let running = self.running.clone();

        let scheduled = self.scheduler.schedule(move || async move {
            tracing::debug!(thread = ?std::thread::current().name(), "processing worker call");

            let kernel = K::create(offscreen_kernel_config);
            let context = SchedulerContext::new(sender, waker);
            #[cfg(target_arch = "wasm32")]
            let flush_context = context.clone();
            // A result that cannot be delivered, because the map already shut down, must
            // not take the worker thread down with it.
            let call = procedure(input, context, kernel);
            match Abortable::new(call, registration).await {
                Ok(Err(ProcedureError::Send(_))) => {
                    tracing::debug!(attempt, "discarding worker result after map shutdown")
                }
                Ok(Err(error)) => tracing::warn!(?error, "procedure failed"),
                Ok(Ok(())) => {
                    #[cfg(target_arch = "wasm32")]
                    let _ = flush_context.flush();
                }
                Err(Aborted) => tracing::debug!(attempt, "cancelled worker call stopped"),
            }
            crate::platform::lock_unpoisoned(&running).remove(&attempt);
        });
        if scheduled.is_err() {
            // A rejected factory never runs, so nothing else removes its entry.
            self.running().remove(&attempt);
        }
        scheduled.map_err(CallError::Schedule)
    }

    fn has_arrivals(&self) -> bool {
        self.batch_open.set(true);
        let mut queued = self.queued.borrow_mut();
        if !queued.is_empty() {
            return true;
        }
        if let Ok(message) = self.channel.1.try_recv() {
            queued.push_back(message);
            return true;
        }
        false
    }

    fn cancel(&self, attempt: u64) {
        if let Some(abort) = self.running().remove(&attempt) {
            abort.abort();
        }
    }

    fn image_providers(&self) -> Option<&crate::sdf::assets::ImageProviders> {
        Some(&self.offscreen_kernel_config.image_providers)
    }

    fn supports_waker(&self) -> bool {
        true
    }

    fn set_waker(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        self.scheduler.set_main_thread_waker(wake.clone());
        let wrapped = self.scheduler.wrap_worker_waker(wake);
        *crate::platform::lock_unpoisoned(&self.waker) = Some(wrapped);
    }
}
