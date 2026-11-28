//! Tracked task dispatch wrapper for [`SchedulerAsyncProcedureCall`](super::SchedulerAsyncProcedureCall).

use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc::Sender,
    Arc, Mutex,
};

use futures::future::{AbortHandle, AbortRegistration, Abortable, Aborted};
use rustc_hash::FxHashMap;

use super::{ArrivalWaker, SchedulerContext};
use crate::io::apc::{
    AsyncProcedure, Input, Message, OffscreenKernel, OffscreenKernelConfig, ProcedureError,
};

pub(super) type RunningCalls = Arc<Mutex<FxHashMap<u64, (AbortHandle, Arc<AtomicBool>)>>>;

pub(super) struct CallDispatch<K: OffscreenKernel> {
    pub(super) input: Input,
    pub(super) procedure: AsyncProcedure<K, SchedulerContext>,
    pub(super) config: OffscreenKernelConfig,
    pub(super) sender: Sender<Message>,
    pub(super) batch_sender: Sender<Vec<Message>>,
    pub(super) waker: Option<ArrivalWaker>,
    pub(super) registration: AbortRegistration,
    pub(super) cancel_flag: Arc<AtomicBool>,
    pub(super) sent_any: Arc<AtomicBool>,
    pub(super) running: RunningCalls,
    pub(super) in_flight: Arc<AtomicUsize>,
    pub(super) active_calls: Arc<AtomicUsize>,
}

struct ActiveCallGuard(Arc<AtomicUsize>);

impl Drop for ActiveCallGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(super) fn register_running_call(
    running: &Mutex<FxHashMap<u64, (AbortHandle, Arc<AtomicBool>)>>,
    in_flight: &AtomicUsize,
    attempt: u64,
    abort: AbortHandle,
    cancel_flag: Arc<AtomicBool>,
) {
    let mut map = crate::util::sync::lock_unpoisoned(running);
    map.insert(attempt, (abort, cancel_flag));
    in_flight.store(map.len(), Ordering::Release);
}

pub(super) fn remove_running_call(
    running: &Mutex<FxHashMap<u64, (AbortHandle, Arc<AtomicBool>)>>,
    in_flight: &AtomicUsize,
    attempt: u64,
) -> (Option<(AbortHandle, Arc<AtomicBool>)>, usize) {
    let mut map = crate::util::sync::lock_unpoisoned(running);
    let entry = map.remove(&attempt);
    let len = map.len();
    in_flight.store(len, Ordering::Release);
    (entry, len)
}

pub(super) fn remove_running_batch(
    running: &Mutex<FxHashMap<u64, (AbortHandle, Arc<AtomicBool>)>>,
    in_flight: &AtomicUsize,
    attempts: &[u64],
) -> Vec<(AbortHandle, Arc<AtomicBool>)> {
    let mut aborts = Vec::with_capacity(attempts.len());
    let mut map = crate::util::sync::lock_unpoisoned(running);
    for &attempt in attempts {
        if let Some(entry) = map.remove(&attempt) {
            aborts.push(entry);
        }
    }
    in_flight.store(map.len(), Ordering::Release);
    aborts
}

pub(super) fn abort_running_call(abort: AbortHandle, flag: Arc<AtomicBool>) {
    flag.store(true, Ordering::Release);
    abort.abort();
}

fn is_flag_set(flag: &AtomicBool) -> bool {
    flag.load(Ordering::Acquire)
}

fn wake_if_drained_silent(
    was_tracked: bool,
    remaining: usize,
    sent_any: &AtomicBool,
    waker: Option<Arc<dyn Fn() + Send + Sync>>,
) {
    if was_tracked && remaining == 0 && !is_flag_set(sent_any) {
        if let Some(wake) = waker {
            wake();
        }
    }
}

pub(super) async fn run_tracked_call<K: OffscreenKernel>(dispatch: CallDispatch<K>) {
    let _active_guard = ActiveCallGuard(dispatch.active_calls);
    if is_flag_set(&dispatch.cancel_flag) {
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
    // A result that cannot be delivered because the map shut down must not panic the worker.
    let call = (dispatch.procedure)(dispatch.input, context, kernel);
    let outcome = Abortable::new(call, dispatch.registration).await;
    let (removed, remaining) = remove_running_call(&dispatch.running, &dispatch.in_flight, attempt);
    let was_tracked = removed.is_some();
    match outcome {
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
    wake_if_drained_silent(was_tracked, remaining, &dispatch.sent_any, dispatch.waker);
}
