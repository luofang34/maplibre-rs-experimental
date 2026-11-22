//! Scheduler-backed [`AsyncProcedureCall`] and [`SchedulerContext`] implementation.

use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    marker::PhantomData,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc, Mutex,
    },
    vec::IntoIter,
};

use futures::future::{AbortHandle, AbortRegistration, Abortable, Aborted};
use rustc_hash::{FxHashMap, FxHashSet};

mod context;
pub use context::SchedulerContext;

use super::{AsyncProcedure, AsyncProcedureCall, CallError, Input, Message, ProcedureError};
use crate::{
    environment::{OffscreenKernel, OffscreenKernelConfig},
    io::scheduler::Scheduler,
};

#[cfg(target_arch = "wasm32")]
const MAX_ADMITTED_ATTEMPTS_PER_BATCH: usize = 4;

pub(crate) type ArrivalWaker = Arc<dyn Fn() + Send + Sync>;
type SharedArrivalWaker = Arc<Mutex<Option<ArrivalWaker>>>;

struct CallDispatch<K: OffscreenKernel> {
    input: Input,
    procedure: AsyncProcedure<K, SchedulerContext>,
    config: OffscreenKernelConfig,
    sender: Sender<Message>,
    batch_sender: Sender<Vec<Message>>,
    waker: Option<ArrivalWaker>,
    registration: AbortRegistration,
    cancel_flag: Arc<AtomicBool>,
    sent_any: Arc<AtomicBool>,
    running: Arc<Mutex<FxHashMap<u64, (AbortHandle, Arc<AtomicBool>)>>>,
    in_flight: Arc<AtomicUsize>,
}

async fn run_tracked_call<K: OffscreenKernel>(dispatch: CallDispatch<K>) {
    if dispatch.cancel_flag.load(Ordering::Acquire) {
        tracing::debug!(
            attempt = dispatch.input.attempt,
            "queued worker call cancelled before start"
        );
        return;
    }
    tracing::debug!(thread = ?std::thread::current().name(), "processing worker call");

    let attempt = dispatch.input.attempt;
    let kernel = K::create(dispatch.config);
    let context = SchedulerContext::for_call(
        dispatch.sender,
        dispatch.batch_sender,
        dispatch.waker.clone(),
        dispatch.cancel_flag,
        dispatch.sent_any.clone(),
    );
    #[cfg(target_arch = "wasm32")]
    let flush_context = context.clone();
    // A result that cannot be delivered, because the map already shut down, must
    // not take the worker thread down with it.
    let call = (dispatch.procedure)(dispatch.input, context, kernel);
    match Abortable::new(call, dispatch.registration).await {
        Ok(Err(ProcedureError::Send(_))) => {
            tracing::debug!(attempt, "discarding worker result after map shutdown")
        }
        Ok(Err(error)) => tracing::warn!(?error, "procedure failed"),
        Ok(Ok(())) => {
            #[cfg(target_arch = "wasm32")]
            drop(flush_context.flush());
        }
        Err(Aborted) => tracing::debug!(attempt, "cancelled worker call stopped"),
    }
    let remaining = {
        let mut map = crate::platform::lock_unpoisoned(&dispatch.running);
        map.remove(&attempt);
        let len = map.len();
        dispatch.in_flight.store(len, Ordering::Release);
        len
    };
    if remaining == 0 && !dispatch.sent_any.load(Ordering::Acquire) {
        if let Some(wake) = dispatch.waker {
            wake();
        }
    }
}

/// An APC using a scheduler for execution and an in-process channel for replies.
///
/// Receiving drains available matches in delivery order. Procedure failures are logged;
/// only preparation and admission failures are returned by [`AsyncProcedureCall::call`].
pub struct SchedulerAsyncProcedureCall<K: OffscreenKernel, S: Scheduler> {
    pub(crate) channel: (Sender<Message>, Receiver<Message>),
    batch_channel: (Sender<Vec<Message>>, Receiver<Vec<Message>>),
    /// Admitted messages retained for sibling consumers in the current frame.
    buffer: RefCell<Vec<Message>>,
    /// Unadmitted arrivals peeked from channels or deferred to a subsequent frame.
    queued: RefCell<VecDeque<Message>>,
    /// Whether `receive` may admit new messages from channels during the current frame.
    batch_open: Cell<bool>,
    /// When `true` (initial boot), `receive` drains all ready worker results without batch caps.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    boot_unthrottled: Cell<bool>,
    scheduler: S,
    phantom_k: PhantomData<K>,
    offscreen_kernel_config: OffscreenKernelConfig,
    /// Tracked calls still running, by attempt, so cancelled calls can be stopped and skipped.
    running: Arc<Mutex<FxHashMap<u64, (AbortHandle, Arc<AtomicBool>)>>>,
    in_flight: Arc<AtomicUsize>,
    cancelled: RefCell<FxHashSet<u64>>,
    pub(crate) waker: SharedArrivalWaker,
}

impl<K: OffscreenKernel, S: Scheduler> SchedulerAsyncProcedureCall<K, S> {
    #[cfg(test)]
    pub(super) fn running(
        &self,
    ) -> std::sync::MutexGuard<'_, FxHashMap<u64, (AbortHandle, Arc<AtomicBool>)>> {
        crate::platform::lock_unpoisoned(&self.running)
    }

    /// Creates an empty reply channel and keeps the worker kernel configuration for future calls.
    pub fn new(scheduler: S, offscreen_kernel_config: OffscreenKernelConfig) -> Self {
        Self {
            channel: mpsc::channel(),
            batch_channel: mpsc::channel(),
            buffer: RefCell::new(Vec::new()),
            queued: RefCell::new(VecDeque::new()),
            batch_open: Cell::new(true),
            boot_unthrottled: Cell::new(true),
            phantom_k: PhantomData,
            scheduler,
            offscreen_kernel_config,
            running: Arc::default(),
            in_flight: Arc::default(),
            cancelled: RefCell::default(),
            waker: Arc::default(),
        }
    }

    fn drain_channels(&self, queued: &mut VecDeque<Message>) {
        let cancelled = self.cancelled.borrow();
        let keep = |msg: &Message| !msg.attempt().is_some_and(|id| cancelled.contains(&id));
        while let Ok(batch) = self.batch_channel.1.try_recv() {
            for message in batch {
                if keep(&message) {
                    queued.push_back(message);
                }
            }
        }
        while let Ok(message) = self.channel.1.try_recv() {
            if keep(&message) {
                queued.push_back(message);
            }
        }
        drop(cancelled);
        if queued.is_empty()
            && self.in_flight.load(Ordering::Acquire) == 0
            && self.buffer.try_borrow().is_ok_and(|b| b.is_empty())
        {
            self.cancelled.borrow_mut().clear();
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn receive_wasm<F: FnMut(&Message) -> bool>(
        &self,
        mut filter: F,
        ret: &mut Vec<Message>,
        buffer: &mut Vec<Message>,
    ) {
        let mut queued = self.queued.borrow_mut();
        self.drain_channels(&mut queued);
        if self.boot_unthrottled.get() {
            while let Some(message) = queued.pop_front() {
                push_filtered(message, &mut filter, ret, buffer);
            }
            return;
        }
        if queued.is_empty() {
            return;
        }
        let mut admitted_attempts = FxHashSet::with_capacity_and_hasher(
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
            push_filtered(message, &mut filter, ret, buffer);
        }
        if !deferred.is_empty() {
            self.batch_open.set(false);
        }
        *queued = deferred;
    }
}

fn push_filtered<F: FnMut(&Message) -> bool>(
    message: Message,
    filter: &mut F,
    ret: &mut Vec<Message>,
    buffer: &mut Vec<Message>,
) {
    if filter(&message) {
        ret.push(message);
    } else {
        buffer.push(message);
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
        self.receive_wasm(filter, &mut ret, &mut buffer);

        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut queued = self.queued.borrow_mut();
            self.drain_channels(&mut queued);
            while let Some(message) = queued.pop_front() {
                tracing::debug!("Data reached main thread: {message:?}");
                push_filtered(message, &mut filter, &mut ret, &mut buffer);
            }
        }

        ret.into_iter()
    }

    fn call(
        &self,
        input: Input,
        procedure: AsyncProcedure<K, Self::Context>,
    ) -> Result<(), CallError> {
        let waker: Option<ArrivalWaker> =
            if let Some(wake) = crate::platform::lock_unpoisoned(&self.waker).clone() {
                Some(wake)
            } else {
                let waker_slot = self.waker.clone();
                Some(Arc::new(move || {
                    if let Some(wake) = crate::platform::lock_unpoisoned(&waker_slot).clone() {
                        wake();
                    }
                }))
            };
        let attempt = input.attempt;
        let (abort, registration) = AbortHandle::new_pair();
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let sent_any = Arc::new(AtomicBool::new(false));
        {
            let mut map = crate::platform::lock_unpoisoned(&self.running);
            map.insert(attempt, (abort, cancel_flag.clone()));
            self.in_flight.store(map.len(), Ordering::Release);
        }
        let dispatch = CallDispatch {
            input,
            procedure,
            config: self.offscreen_kernel_config.clone(),
            sender: self.channel.0.clone(),
            batch_sender: self.batch_channel.0.clone(),
            waker,
            registration,
            cancel_flag,
            sent_any,
            running: self.running.clone(),
            in_flight: self.in_flight.clone(),
        };
        let scheduled = self.scheduler.schedule(move || run_tracked_call(dispatch));
        if scheduled.is_err() {
            // A rejected factory never runs, so nothing else removes its entry.
            let mut map = crate::platform::lock_unpoisoned(&self.running);
            map.remove(&attempt);
            self.in_flight.store(map.len(), Ordering::Release);
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
        self.drain_channels(&mut queued);
        !queued.is_empty()
    }

    fn cancel(&self, attempt: u64) {
        self.cancelled.borrow_mut().insert(attempt);
        let removed = {
            let mut map = crate::platform::lock_unpoisoned(&self.running);
            let entry = map.remove(&attempt);
            self.in_flight.store(map.len(), Ordering::Release);
            entry
        };
        if let Some((abort, flag)) = removed {
            flag.store(true, Ordering::Release);
            abort.abort();
        }
    }

    fn cancel_batch(&self, attempts: &[u64]) {
        if attempts.is_empty() {
            return;
        }
        self.cancelled.borrow_mut().extend(attempts.iter().copied());
        let mut aborts = Vec::with_capacity(attempts.len());
        {
            let mut map = crate::platform::lock_unpoisoned(&self.running);
            for &attempt in attempts {
                if let Some(entry) = map.remove(&attempt) {
                    aborts.push(entry);
                }
            }
            self.in_flight.store(map.len(), Ordering::Release);
        }
        for (abort, flag) in aborts {
            flag.store(true, Ordering::Release);
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
        self.in_flight.load(Ordering::Acquire)
    }

    fn warm_style(&self, style: &crate::style::Style) {
        #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
        {
            let plan = crate::sdf::assets::PrefetchPlan::from_style(style);
            if !plan.is_empty() {
                let config = self.offscreen_kernel_config.clone();
                drop(self.scheduler.schedule(move || async move {
                    let kernel = K::create(config);
                    let client = kernel.source_client();
                    plan.execute_decoded_wasm(&client).await;
                }));
            }
        }
        #[cfg(not(all(target_arch = "wasm32", target_feature = "atomics")))]
        let _ = style;
    }

    fn set_boot_unthrottled(&self, unthrottled: bool) {
        self.boot_unthrottled.set(unthrottled);
    }
}
