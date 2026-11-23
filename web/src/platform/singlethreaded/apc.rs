//! Main-thread transport for tile calls served by single-threaded web workers.

// A failure here must reach the map as a typed error: a panic on the render thread stops it.
#![deny(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

#[cfg(any(test, feature = "renderer"))]
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};

#[cfg(any(test, feature = "renderer"))]
use maplibre::{
    environment::{OffscreenKernel, OffscreenKernelConfig},
    io::{
        apc::{AsyncProcedure, AsyncProcedureCall, CallError, Input, Message, ProcedureTag},
        scheduler::ScheduleError,
    },
};
#[cfg(any(test, feature = "renderer"))]
use wasm_bindgen::JsValue;

#[cfg(any(test, feature = "renderer"))]
pub use self::raw_decode::PassingReceiveIter;
pub use self::tag::WebMessageTag;
#[cfg(test)]
pub(crate) use self::wire::{encode_call_finished, prepare_message};
pub(crate) use self::wire::{post_call_finished, set_worker_reply_port};
#[cfg(any(test, feature = "renderer"))]
use self::{assignments::Assignments, style_revisions::StyleRevisions};
#[cfg(any(test, feature = "renderer"))]
pub(crate) use self::{
    raw_queue::QueuedMessage,
    wire::{decode_call_finished, decode_message, TrackedPayloadError},
};
#[cfg(any(test, feature = "renderer"))]
use crate::{error::WebError, platform::singlethreaded::UsedContext};

/// What the workers sent since the last receive.
#[cfg(any(test, feature = "renderer"))]
#[derive(Default)]
pub struct Received {
    /// Pre-decoded results (used by unit tests), in delivery order.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) messages: Vec<Message>,
    /// Wire payloads held in JS `ArrayBuffer`s until a consumer pulls them one at a time.
    pub(crate) queued: Vec<QueuedMessage>,
    /// Attempts whose calls a worker stopped running.
    pub(crate) finished: Vec<u64>,
    /// Attempts cancelled on the main thread so late worker replies can be dropped before copying.
    pub(crate) cancelled: rustc_hash::FxHashSet<u64>,
    /// Main-thread redraw waker invoked when results transition from empty to non-empty.
    pub(crate) waker: Option<Arc<dyn Fn() + Send + Sync>>,
}

#[cfg(any(test, feature = "renderer"))]
pub type ReceivedType = RefCell<Received>;

#[cfg(any(test, feature = "renderer"))]
pub struct PassingAsyncProcedureCall {
    workers: Vec<recycle::WorkerEndpoint>,
    listeners: Vec<recycle::WorkerMessageClosure>,

    buffer: RefCell<Vec<QueuedMessage>>,
    scratch: RefCell<scratch::ReceiveScratch>,
    outgoing: RefCell<outgoing::OutgoingCallBatches>,
    batch_open: Cell<bool>,
    boot_unthrottled: Cell<bool>,

    received: Rc<ReceivedType>,

    /// The worker each running call was sent to, so a cancel reaches it.
    assignments: RefCell<Assignments>,

    /// The style the workers hold, so calls name it instead of carrying it.
    styles: RefCell<StyleRevisions>,
}

#[cfg(any(test, feature = "renderer"))]
impl PassingAsyncProcedureCall {
    pub fn new(
        new_worker: js_sys::Function,
        initial_workers: usize,
        config: OffscreenKernelConfig,
    ) -> Result<Self, WebError> {
        let received = Rc::new(RefCell::new(Received::default()));
        let config_json = serde_json::to_string(&config).map_err(WebError::KernelConfigEncoding)?;
        let (workers, listeners) =
            recycle::acquire_workers(&new_worker, initial_workers, &config_json, &received)?;

        Ok(Self {
            workers,
            listeners,
            buffer: RefCell::new(Vec::default()),
            scratch: RefCell::default(),
            outgoing: RefCell::default(),
            batch_open: Cell::new(true),
            boot_unthrottled: Cell::new(true),
            received,
            assignments: RefCell::default(),
            styles: RefCell::default(),
        })
    }

    fn flush_calls_inner(&self) {
        let mut assignments = self.assignments.borrow_mut();
        self.outgoing
            .borrow_mut()
            .flush(&self.workers, &mut assignments);
    }

    fn post_style_to_workers(&self, revision: u64, style: &str) -> Result<(), WebError> {
        for (idx, worker) in self.workers.iter().enumerate() {
            post::post_st_style(worker.as_js_value(), revision, style, idx == 0)
                .map_err(WebError::from)?;
        }
        Ok(())
    }

    fn dispatch_tagged(&self, tag: ProcedureTag, input: Input) -> Result<(), CallError> {
        let attempt = input.attempt;
        if self.workers.is_empty() {
            return Err(CallError::Schedule(ScheduleError::NoWorkers));
        }
        // The input leaves its style out; the workers hold it, sent once per revision.
        let style_revision = self
            .styles
            .borrow_mut()
            .revision(&input.style, |revision, style| {
                self.flush_calls_inner();
                self.post_style_to_workers(revision, style)
            })
            .map_err(|error| CallError::Serialize(Box::new(error)))?;

        if let Ok(mut received) = self.received.try_borrow_mut() {
            let mut assignments = self.assignments.borrow_mut();
            for finished_attempt in received.finished.drain(..) {
                assignments.finished(finished_attempt);
            }
        }
        let index = self
            .assignments
            .borrow_mut()
            .least_loaded_worker(self.workers.len());
        let worker = self
            .workers
            .get(index)
            .ok_or(CallError::Schedule(ScheduleError::NoWorkers))?;

        if input.only_sources.is_some() {
            let serialized =
                serde_json::to_string(&input).map_err(|e| CallError::Serialize(Box::new(e)))?;
            let message = recycle::worker_message(
                "call",
                &[
                    ("procedure_tag", &JsValue::from(tag.as_u8())),
                    ("attempt", &JsValue::from(attempt)),
                    ("style_revision", &JsValue::from(style_revision)),
                    ("input", &JsValue::from(serialized)),
                ],
            )
            .map_err(|error| CallError::Serialize(Box::new(error)))?;
            worker.post_message(&message).map_err(|error| {
                CallError::Schedule(ScheduleError::Scheduling(Box::new(WebError::from(error))))
            })?;
        } else {
            let meta = (tag.as_u8() as u32)
                | ((u8::from(input.coords.z) as u32) << 8)
                | ((input.overscaled_zoom as u32) << 16)
                | (u32::from(input.refetch) << 24);
            self.outgoing.borrow_mut().push(
                index,
                self.workers.len(),
                outgoing::PackedTileCall {
                    attempt,
                    style_revision,
                    x: input.coords.x,
                    y: input.coords.y,
                    meta,
                    pixel_ratio: input.pixel_ratio,
                },
            );
        }
        self.assignments.borrow_mut().assign(attempt, index);
        Ok(())
    }
}

#[cfg(any(test, feature = "renderer"))]
impl PassingAsyncProcedureCall {
    fn drain_finished(&self, received: &mut Received) {
        if !received.finished.is_empty() {
            let mut assignments = self.assignments.borrow_mut();
            for attempt in received.finished.drain(..) {
                assignments.finished(attempt);
            }
        }
    }

    fn admit_throttled_queued<F: FnMut(&Message) -> bool>(
        &self,
        received: &mut Received,
        buffer: &mut Vec<QueuedMessage>,
        ret: &mut Vec<QueuedMessage>,
        filter: &mut F,
    ) {
        let mut scratch = self.scratch.borrow_mut();
        let scratch = &mut *scratch;
        scratch.budget.reset_from_buffer(buffer);
        scratch.attempt_totals.clear();
        for item in &received.queued {
            if let Some(id) = item.attempt() {
                let entry = scratch.attempt_totals.entry(id).or_insert(0);
                *entry = entry.saturating_add(item.raw_byte_len());
            }
        }
        scratch.deferred.clear();
        for item in received.queued.drain(..) {
            if !scratch.budget.try_admit(&item, &scratch.attempt_totals) {
                scratch.deferred.push(item);
                continue;
            }
            if item.matches(filter) {
                ret.push(item);
            } else {
                buffer.push(item);
            }
        }
        if !scratch.deferred.is_empty() {
            self.batch_open.set(false);
        }
        std::mem::swap(&mut received.queued, &mut scratch.deferred);
    }
}

#[cfg(any(test, feature = "renderer"))]
impl<K: OffscreenKernel> AsyncProcedureCall<K> for PassingAsyncProcedureCall {
    type Context = UsedContext;
    type ReceiveIterator<F: FnMut(&Message) -> bool> = PassingReceiveIter;

    fn receive<F: FnMut(&Message) -> bool>(&self, mut filter: F) -> Self::ReceiveIterator<F> {
        let mut buffer = self.buffer.borrow_mut();
        if buffer.is_empty() {
            let Ok(mut received) = self.received.try_borrow_mut() else {
                return PassingReceiveIter::new(Vec::new(), None);
            };
            self.drain_finished(&mut received);
            if received.messages.is_empty()
                && (received.queued.is_empty()
                    || (!self.boot_unthrottled.get() && !self.batch_open.get()))
            {
                return PassingReceiveIter::new(Vec::new(), None);
            }
        }
        // Partial and final tile completions must retain the worker's delivery order.
        let mut ret: Vec<QueuedMessage> = buffer
            .extract_if(.., |item| item.matches(&mut filter))
            .collect();

        let weak = Some(Rc::downgrade(&self.received));
        let Ok(mut received) = self.received.try_borrow_mut() else {
            return PassingReceiveIter::new(ret, weak);
        };
        self.drain_finished(&mut received);
        for message in received.messages.drain(..) {
            if filter(&message) {
                ret.push(QueuedMessage::Decoded(message));
            } else {
                buffer.push(QueuedMessage::Decoded(message));
            }
        }
        if received.queued.is_empty() {
            return PassingReceiveIter::new(ret, weak);
        }
        if self.boot_unthrottled.get() {
            for item in received.queued.drain(..) {
                if item.matches(&mut filter) {
                    ret.push(item);
                } else {
                    buffer.push(item);
                }
            }
            return PassingReceiveIter::new(ret, weak);
        }
        if self.batch_open.get() {
            self.admit_throttled_queued(&mut received, &mut buffer, &mut ret, &mut filter);
        }
        PassingReceiveIter::new(ret, weak)
    }

    fn call(
        &self,
        _input: Input,
        _procedure: AsyncProcedure<K, UsedContext>,
    ) -> Result<(), CallError> {
        Err(CallError::Schedule(ScheduleError::Scheduling(Box::new(
            WebError::InvalidMessage,
        ))))
    }

    fn call_tagged(
        &self,
        tag: ProcedureTag,
        input: Input,
        _procedure: AsyncProcedure<K, UsedContext>,
    ) -> Result<(), CallError> {
        self.dispatch_tagged(tag, input)
    }

    fn flush_calls(&self) {
        self.flush_calls_inner();
    }

    fn supports_prewarm(&self) -> bool {
        true
    }

    fn prewarm_call(&self, tag: ProcedureTag, input: Input) -> Result<(), CallError> {
        self.dispatch_tagged(tag, input)
    }

    fn cancel(&self, attempt: u64) {
        let Some(index) = self.assignments.borrow_mut().cancel(attempt) else {
            return;
        };
        if self.outgoing.borrow_mut().remove_pending(index, attempt) {
            return;
        }
        if let Ok(mut received) = self.received.try_borrow_mut() {
            if received.cancelled.len() > 2048 {
                received.cancelled.clear();
            }
            received.cancelled.insert(attempt);
        }
        let Some(worker) = self.workers.get(index) else {
            return;
        };
        if let Err(error) =
            post::post_st_cancel(worker.as_js_value(), attempt).map_err(WebError::from)
        {
            // The call finishes and the map ignores its result.
            tracing::warn!(attempt, %error, "cancelling a worker call failed");
        }
    }

    fn cancel_batch(&self, attempts: &[u64]) {
        if attempts.is_empty() || self.workers.is_empty() {
            return;
        }
        let mut scratch = self.scratch.borrow_mut();
        if scratch.cancel_buckets.len() < self.workers.len() {
            scratch
                .cancel_buckets
                .resize_with(self.workers.len(), Vec::new);
        }
        for bucket in &mut scratch.cancel_buckets {
            bucket.clear();
        }
        {
            let mut outgoing = self.outgoing.borrow_mut();
            let mut assignments = self.assignments.borrow_mut();
            let mut received = self.received.try_borrow_mut().ok();
            for &attempt in attempts {
                if let Some(index) = assignments.cancel(attempt) {
                    if outgoing.remove_pending(index, attempt) {
                        continue;
                    }
                    if let Some(ref mut rec) = received {
                        if rec.cancelled.len() > 2048 {
                            rec.cancelled.clear();
                        }
                        rec.cancelled.insert(attempt);
                    }
                    if let Some(bucket) = scratch.cancel_buckets.get_mut(index) {
                        bucket.push(attempt);
                    }
                }
            }
        }
        for (index, bucket) in scratch.cancel_buckets.iter().enumerate() {
            if bucket.is_empty() {
                continue;
            }
            let Some(worker) = self.workers.get(index) else {
                continue;
            };
            let res = if let [only] = bucket.as_slice() {
                post::post_st_cancel(worker.as_js_value(), *only)
            } else {
                post::post_st_cancel_batch(worker.as_js_value(), bucket.as_slice())
            };
            if let Err(error) = res.map_err(WebError::from) {
                tracing::warn!(count = bucket.len(), %error, "batch-cancelling worker calls failed");
            }
        }
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
        self.received
            .try_borrow()
            .is_ok_and(|received| !received.messages.is_empty() || !received.queued.is_empty())
    }

    fn supports_waker(&self) -> bool {
        true
    }

    fn set_waker(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        if let Ok(mut received) = self.received.try_borrow_mut() {
            received.waker = Some(wake);
        }
    }

    fn warm_style(&self, style: &maplibre::style::Style) {
        if self.workers.is_empty() {
            return;
        }
        self.flush_calls_inner();
        if let Err(error) = self.styles.borrow_mut().warm(style, |revision, json| {
            self.post_style_to_workers(revision, json)
        }) {
            tracing::warn!(%error, "pre-warming worker style failed");
        }
    }

    fn in_flight_calls(&self) -> usize {
        let Ok(mut assignments) = self.assignments.try_borrow_mut() else {
            return 1;
        };
        if let Ok(mut received) = self.received.try_borrow_mut() {
            for attempt in received.finished.drain(..) {
                assignments.finished(attempt);
            }
        }
        assignments.len()
    }

    fn set_boot_unthrottled(&self, unthrottled: bool) {
        self.boot_unthrottled.set(unthrottled);
    }
}

#[cfg(any(test, feature = "renderer"))]
impl Drop for PassingAsyncProcedureCall {
    fn drop(&mut self) {
        self.outgoing.get_mut().clear();
        let mut busy = vec![false; self.workers.len()];
        for (_, idx) in self.assignments.get_mut().drain() {
            if let Some(slot) = busy.get_mut(idx) {
                *slot = true;
            }
        }
        let workers = std::mem::take(&mut self.workers);
        let listeners = std::mem::take(&mut self.listeners);
        recycle::release_workers(workers, listeners, &busy);
        self.buffer.get_mut().clear();
        if let Ok(mut received) = self.received.try_borrow_mut() {
            received.messages.clear();
            received.queued.clear();
            received.finished.clear();
            received.cancelled.clear();
            received.waker = None;
        }
        crate::platform::singlethreaded::transferables::clear_symbol_atlas_cache();
    }
}

#[cfg(any(test, feature = "renderer"))]
mod assignments;
#[cfg(any(test, feature = "renderer"))]
mod outgoing;
#[cfg(any(test, feature = "renderer"))]
mod post;
#[cfg(any(test, feature = "renderer"))]
mod raw_decode;
#[cfg(any(test, feature = "renderer"))]
mod raw_queue;
#[cfg(any(test, feature = "renderer"))]
mod recycle;
#[cfg(any(test, feature = "renderer"))]
mod scratch;
#[cfg(any(test, feature = "renderer"))]
mod style_revisions;
pub mod tag;
#[cfg(test)]
mod tests;
pub(crate) mod wire;
