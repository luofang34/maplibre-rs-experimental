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

use futures::future::AbortHandle;
use rustc_hash::FxHashSet;

mod context;
mod dispatch;
pub use context::SchedulerContext;
use dispatch::{
    abort_running_call, register_running_call, remove_running_batch, remove_running_call,
    run_tracked_call, CallDispatch, RunningCalls,
};

use super::{
    AsyncProcedure, AsyncProcedureCall, CallError, Input, Message, OffscreenKernel,
    OffscreenKernelConfig,
};
use crate::io::scheduler::Scheduler;

#[cfg(target_arch = "wasm32")]
const MAX_ADMITTED_ATTEMPTS_PER_BATCH: usize = 4;

pub(crate) type ArrivalWaker = Arc<dyn Fn() + Send + Sync>;
type SharedArrivalWaker = Arc<Mutex<Option<ArrivalWaker>>>;

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
    running: RunningCalls,
    in_flight: Arc<AtomicUsize>,
    active_calls: Arc<AtomicUsize>,
    cancelled: RefCell<FxHashSet<u64>>,
    pub(crate) waker: SharedArrivalWaker,
    style_warmer: Option<fn(&crate::style::Style, OffscreenKernelConfig)>,
}

impl<K: OffscreenKernel, S: Scheduler> SchedulerAsyncProcedureCall<K, S> {
    #[cfg(test)]
    pub(super) fn running(
        &self,
    ) -> std::sync::MutexGuard<'_, rustc_hash::FxHashMap<u64, (AbortHandle, Arc<AtomicBool>)>> {
        crate::util::sync::lock_unpoisoned(&self.running)
    }

    #[cfg(test)]
    pub(super) fn cancelled_len(&self) -> usize {
        self.cancelled.borrow().len()
    }

    #[cfg(test)]
    pub(super) fn batch_open(&self) -> bool {
        self.batch_open.get()
    }

    #[cfg(test)]
    pub(super) fn set_batch_open_for_test(&self, open: bool) {
        self.batch_open.set(open);
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
            active_calls: Arc::default(),
            cancelled: RefCell::default(),
            waker: Arc::default(),
            style_warmer: None,
        }
    }

    /// Registers a hook invoked when [`AsyncProcedureCall::warm_style`] runs on a shared-memory
    /// worker pool.
    #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
    pub fn with_style_warmer(
        mut self,
        warmer: fn(&crate::style::Style, OffscreenKernelConfig),
    ) -> Self {
        self.style_warmer = Some(warmer);
        self
    }

    fn drain_channels(&self, queued: &mut VecDeque<Message>) {
        let cancelled = self.cancelled.borrow();
        let keep = |msg: &Message| !msg.attempt().is_some_and(|id| cancelled.contains(&id));
        drain_batch_queue(&self.batch_channel.1, queued, keep);
        drain_single_queue(&self.channel.1, queued, keep);
        drop(cancelled);
        if queued.is_empty()
            && all_work_finished(&self.in_flight, &self.active_calls)
            && is_buffer_empty(&self.buffer)
        {
            self.cancelled.borrow_mut().clear();
        }
    }

    #[inline]
    fn is_cancelled(&self, message: &Message) -> bool {
        is_attempt_cancelled(&self.cancelled, message.attempt())
    }

    fn purge_cancelled_local(&self) {
        let mut dropped = Vec::new();
        if let Ok(cancelled) = self.cancelled.try_borrow() {
            if cancelled.is_empty() {
                return;
            }
            prune_cancelled_queue(&self.queued, &cancelled, &mut dropped, Message::attempt);
            prune_cancelled_buffer(&self.buffer, &cancelled, &mut dropped, Message::attempt);
        }
        drop(dropped);
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
                if self.is_cancelled(&message) {
                    continue;
                }
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
            if self.is_cancelled(&message) {
                continue;
            }
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
                if self.is_cancelled(&message) {
                    continue;
                }
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
        let waker = Some(resolve_call_waker(&self.waker));
        let attempt = input.attempt;
        let (abort, registration) = AbortHandle::new_pair();
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let sent_any = Arc::new(AtomicBool::new(false));
        register_running_call(
            &self.running,
            &self.in_flight,
            attempt,
            abort,
            cancel_flag.clone(),
        );
        self.active_calls.fetch_add(1, Ordering::AcqRel);
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
            active_calls: self.active_calls.clone(),
        };
        let scheduled = self.scheduler.schedule(move || run_tracked_call(dispatch));
        if scheduled.is_err() {
            // A rejected factory never runs, so nothing else removes its entry.
            self.active_calls.fetch_sub(1, Ordering::AcqRel);
            let _ = remove_running_call(&self.running, &self.in_flight, attempt);
        }
        scheduled.map_err(CallError::Schedule)
    }

    fn has_arrivals(&self) -> bool {
        self.batch_open.set(true);
        self.has_pending_arrivals()
    }

    fn has_pending_arrivals(&self) -> bool {
        if has_buffered_items(&self.buffer) {
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
        let (removed, _) = remove_running_call(&self.running, &self.in_flight, attempt);
        if let Some((abort, flag)) = removed {
            abort_running_call(abort, flag);
        }
        self.purge_cancelled_local();
    }

    fn cancel_batch(&self, attempts: &[u64]) {
        if attempts.is_empty() {
            return;
        }
        self.cancelled.borrow_mut().extend(attempts.iter().copied());
        for (abort, flag) in remove_running_batch(&self.running, &self.in_flight, attempts) {
            abort_running_call(abort, flag);
        }
        self.purge_cancelled_local();
    }

    fn supports_prewarm(&self) -> bool {
        cfg!(target_arch = "wasm32")
    }

    fn prewarm_call(
        &self,
        _tag: super::ProcedureTag,
        input: Input,
        procedure: AsyncProcedure<K, Self::Context>,
    ) -> Result<(), CallError> {
        self.call(input, procedure)
    }

    fn image_providers(&self) -> Option<&crate::io::source_client::ImageProviders> {
        Some(&self.offscreen_kernel_config.image_providers)
    }

    fn supports_waker(&self) -> bool {
        true
    }

    fn set_waker(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        self.scheduler.set_main_thread_waker(wake.clone());
        store_wrapped_waker(&self.waker, self.scheduler.wrap_worker_waker(wake));
    }

    fn in_flight_calls(&self) -> usize {
        self.in_flight.load(Ordering::Acquire)
    }

    fn warm_style(&self, style: &crate::style::Style) {
        if let Some(warmer) = self.style_warmer {
            warmer(style, self.offscreen_kernel_config.clone());
        }
    }

    fn set_boot_unthrottled(&self, unthrottled: bool) {
        self.boot_unthrottled.set(unthrottled);
    }
}

fn drain_batch_queue<T>(
    rx: &Receiver<Vec<T>>,
    queued: &mut VecDeque<T>,
    keep: impl Fn(&T) -> bool,
) {
    while let Ok(batch) = rx.try_recv() {
        for item in batch {
            if keep(&item) {
                queued.push_back(item);
            }
        }
    }
}

fn drain_single_queue<T>(rx: &Receiver<T>, queued: &mut VecDeque<T>, keep: impl Fn(&T) -> bool) {
    while let Ok(item) = rx.try_recv() {
        if keep(&item) {
            queued.push_back(item);
        }
    }
}

fn prune_cancelled_queue<T>(
    queued: &RefCell<VecDeque<T>>,
    cancelled: &FxHashSet<u64>,
    dropped: &mut Vec<T>,
    attempt_of: impl Fn(&T) -> Option<u64>,
) {
    if let Ok(mut queued) = queued.try_borrow_mut() {
        let len = queued.len();
        for _ in 0..len {
            if let Some(item) = queued.pop_front() {
                if attempt_of(&item).is_some_and(|id| cancelled.contains(&id)) {
                    dropped.push(item);
                } else {
                    queued.push_back(item);
                }
            }
        }
    }
}

fn prune_cancelled_buffer<T>(
    buffer: &RefCell<Vec<T>>,
    cancelled: &FxHashSet<u64>,
    dropped: &mut Vec<T>,
    attempt_of: impl Fn(&T) -> Option<u64>,
) {
    if let Ok(mut buffer) = buffer.try_borrow_mut() {
        dropped.extend(buffer.extract_if(.., |item| {
            attempt_of(item).is_some_and(|id| cancelled.contains(&id))
        }));
    }
}

fn is_buffer_empty<T>(buffer: &RefCell<Vec<T>>) -> bool {
    buffer.try_borrow().is_ok_and(|b| b.is_empty())
}

fn has_buffered_items<T>(buffer: &RefCell<Vec<T>>) -> bool {
    buffer.try_borrow().is_ok_and(|b| !b.is_empty())
}

fn all_work_finished(in_flight: &AtomicUsize, active_calls: &AtomicUsize) -> bool {
    in_flight.load(Ordering::Acquire) == 0 && active_calls.load(Ordering::Acquire) == 0
}

fn is_attempt_cancelled(cancelled: &RefCell<FxHashSet<u64>>, attempt: Option<u64>) -> bool {
    attempt.is_some_and(|id| cancelled.borrow().contains(&id))
}

fn resolve_call_waker(slot: &SharedArrivalWaker) -> ArrivalWaker {
    if let Some(wake) = crate::util::sync::lock_unpoisoned(slot).clone() {
        return wake;
    }
    let waker_slot = slot.clone();
    Arc::new(move || {
        if let Some(wake) = crate::util::sync::lock_unpoisoned(&waker_slot).clone() {
            wake();
        }
    })
}

fn store_wrapped_waker(slot: &SharedArrivalWaker, waker: ArrivalWaker) {
    *crate::util::sync::lock_unpoisoned(slot) = Some(waker);
}

#[cfg(test)]
mod tests;
