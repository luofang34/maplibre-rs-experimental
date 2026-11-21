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
const MAX_ADMITTED_ATTEMPTS_PER_BATCH: usize = 4;

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

pub(crate) type ArrivalWaker = Arc<dyn Fn() + Send + Sync>;
type SharedArrivalWaker = Arc<Mutex<Option<ArrivalWaker>>>;

/// Reply sender whose clones share the scheduler-backed APC's receiving channel.
#[derive(Clone)]
pub struct SchedulerContext {
    pub(crate) sender: Sender<Message>,
    pub(crate) waker: Option<ArrivalWaker>,
    #[cfg(target_arch = "wasm32")]
    pending: Arc<Mutex<Vec<Message>>>,
}

impl SchedulerContext {
    pub(crate) fn new(sender: Sender<Message>, waker: Option<ArrivalWaker>) -> Self {
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
    /// When `true` (initial boot), `receive` drains all ready worker results without batch caps.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    boot_unthrottled: Cell<bool>,
    scheduler: S,
    phantom_k: PhantomData<K>,
    offscreen_kernel_config: OffscreenKernelConfig,
    /// Tracked calls still running, by attempt, so a cancelled one can be stopped.
    running: Arc<Mutex<HashMap<u64, AbortHandle>>>,
    pub(crate) waker: SharedArrivalWaker,
}

impl<K: OffscreenKernel, S: Scheduler> SchedulerAsyncProcedureCall<K, S> {
    /// Creates an empty reply channel and keeps the worker kernel configuration for future calls.
    pub fn new(scheduler: S, offscreen_kernel_config: OffscreenKernelConfig) -> Self {
        Self {
            channel: mpsc::channel(),
            buffer: RefCell::new(Vec::new()),
            queued: RefCell::new(VecDeque::new()),
            batch_open: Cell::new(true),
            boot_unthrottled: Cell::new(true),
            phantom_k: PhantomData,
            scheduler,
            offscreen_kernel_config,
            running: Arc::default(),
            waker: Arc::default(),
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
            if self.boot_unthrottled.get() {
                let mut queued = self.queued.borrow_mut();
                while let Some(message) = queued.pop_front() {
                    if filter(&message) {
                        ret.push(message);
                    } else {
                        buffer.push(message);
                    }
                }
                while let Ok(message) = self.channel.1.try_recv() {
                    if filter(&message) {
                        ret.push(message);
                    } else {
                        buffer.push(message);
                    }
                }
                return ret.into_iter();
            }
            let mut queued = self.queued.borrow_mut();
            if queued.is_empty() {
                if let Ok(message) = self.channel.1.try_recv() {
                    queued.push_back(message);
                } else {
                    return ret.into_iter();
                }
            }
            while let Ok(message) = self.channel.1.try_recv() {
                queued.push_back(message);
            }
            let mut admitted_attempts = rustc_hash::FxHashSet::with_capacity_and_hasher(
                MAX_ADMITTED_ATTEMPTS_PER_BATCH,
                Default::default(),
            );
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
        let waker: Option<ArrivalWaker> =
            if let Some(wake) = crate::platform::lock_unpoisoned(&self.waker).clone() {
                Some(wake)
            } else {
                let waker_slot = self.waker.clone();
                Some(Arc::new(move || {
                    let current = crate::platform::lock_unpoisoned(&waker_slot).clone();
                    if let Some(wake) = current {
                        wake();
                    }
                }))
            };
        let offscreen_kernel_config = self.offscreen_kernel_config.clone();
        let attempt = input.attempt;
        let (abort, registration) = AbortHandle::new_pair();
        self.running().insert(attempt, abort);
        let running = self.running.clone();
        let finish_waker = waker.clone();

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
            let remaining = {
                let mut map = crate::platform::lock_unpoisoned(&running);
                map.remove(&attempt);
                map.len()
            };
            if remaining == 0 {
                if let Some(wake) = finish_waker {
                    wake();
                }
            }
        });
        if scheduled.is_err() {
            // A rejected factory never runs, so nothing else removes its entry.
            self.running().remove(&attempt);
        }
        scheduled.map_err(CallError::Schedule)
    }

    fn has_arrivals(&self) -> bool {
        self.batch_open.set(true);
        if self
            .buffer
            .try_borrow()
            .is_ok_and(|buffer| !buffer.is_empty())
        {
            return true;
        }
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

    fn supports_prewarm(&self) -> bool {
        cfg!(target_arch = "wasm32")
    }

    fn prewarm_call(&self, tag: super::ProcedureTag, input: Input) -> Result<(), CallError> {
        match tag {
            super::ProcedureTag::FetchVector => self.call(
                input,
                crate::vector::fetch_vector_apc::<
                    K,
                    crate::vector::DefaultVectorTransferables,
                    Self::Context,
                >,
            ),
            #[cfg(feature = "raster")]
            super::ProcedureTag::FetchRaster => self.call(
                input,
                crate::raster::fetch_raster_apc::<
                    K,
                    crate::raster::DefaultRasterTransferables,
                    Self::Context,
                >,
            ),
            #[cfg(not(feature = "raster"))]
            super::ProcedureTag::FetchRaster => Ok(()),
            super::ProcedureTag::FetchDem => self.call(
                input,
                crate::terrain::fetch_dem_apc::<
                    K,
                    crate::terrain::DefaultDemTransferables,
                    Self::Context,
                >,
            ),
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

    fn in_flight_calls(&self) -> usize {
        self.running.try_lock().map_or(1, |running| running.len())
    }

    fn set_boot_unthrottled(&self, unthrottled: bool) {
        self.boot_unthrottled.set(unthrottled);
    }
}
