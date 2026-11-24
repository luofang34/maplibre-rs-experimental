//! Tracked task dispatch wrapper for [`SchedulerAsyncProcedureCall`](super::SchedulerAsyncProcedureCall).

use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc::Sender,
    Arc, Mutex,
};

use futures::future::{AbortHandle, AbortRegistration, Abortable, Aborted};
use rustc_hash::FxHashMap;

use super::{ArrivalWaker, SchedulerContext};
use crate::{
    environment::{OffscreenKernel, OffscreenKernelConfig},
    io::apc::{AsyncProcedure, Input, Message, ProcedureError},
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

pub(super) async fn run_tracked_call<K: OffscreenKernel>(dispatch: CallDispatch<K>) {
    let _active_guard = ActiveCallGuard(dispatch.active_calls);
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
    // A result that cannot be delivered because the map shut down must not panic the worker.
    let call = (dispatch.procedure)(dispatch.input, context, kernel);
    let outcome = Abortable::new(call, dispatch.registration).await;
    let (was_tracked, remaining) = {
        let mut map = crate::platform::lock_unpoisoned(&dispatch.running);
        let was_tracked = map.remove(&attempt).is_some();
        let len = map.len();
        dispatch.in_flight.store(len, Ordering::Release);
        (was_tracked, len)
    };
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
    if was_tracked && remaining == 0 && !dispatch.sent_any.load(Ordering::Acquire) {
        if let Some(wake) = dispatch.waker {
            wake();
        }
    }
}
