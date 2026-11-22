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
pub use self::raw_queue::PassingReceiveIter;
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
            batch_open: Cell::new(true),
            boot_unthrottled: Cell::new(true),
            received,
            assignments: RefCell::default(),
            styles: RefCell::default(),
        })
    }

    fn post_style_to_workers(&self, revision: u64, style: &str) -> Result<(), WebError> {
        let message = worker_message(
            "style",
            &[
                ("revision", &JsValue::from(revision)),
                ("style", &JsValue::from(style)),
            ],
        )?;
        self.workers
            .iter()
            .try_for_each(|worker| worker.post_message(&message).map_err(WebError::from))
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
                self.post_style_to_workers(revision, style)
            })
            .map_err(|error| CallError::Serialize(Box::new(error)))?;
        let input = serde_json::to_string(&input).map_err(|e| CallError::Serialize(Box::new(e)))?;

        // The worker reads the attempt without decoding the input, so a cancel can overtake
        // a call that waits for the worker to start.
        let message = worker_message(
            "call",
            &[
                ("procedure_tag", &JsValue::from(tag.as_u8())),
                ("attempt", &JsValue::from(attempt)),
                ("style_revision", &JsValue::from(style_revision)),
                ("input", &JsValue::from(input)),
            ],
        )
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

        worker.post_message(&message).map_err(|error| {
            CallError::Schedule(ScheduleError::Scheduling(Box::new(WebError::from(error))))
        })?;
        self.assignments.borrow_mut().assign(attempt, index);
        Ok(())
    }
}

/// Builds a worker message `{ type: kind, <key>: value, ... }`.
#[cfg(any(test, feature = "renderer"))]
pub(super) fn worker_message(
    kind: &str,
    fields: &[(&str, &JsValue)],
) -> Result<js_sys::Object, WebError> {
    let entries = js_sys::Array::of1(&js_sys::Array::of2(
        &JsValue::from("type"),
        &JsValue::from(kind),
    ));
    for (key, value) in fields {
        entries.push(&js_sys::Array::of2(&JsValue::from(*key), value));
    }
    js_sys::Object::from_entries(&entries).map_err(WebError::from)
}

#[cfg(any(test, feature = "renderer"))]
struct BatchBudget {
    bytes: usize,
    attempts: rustc_hash::FxHashSet<u64>,
    msgs: usize,
}

#[cfg(any(test, feature = "renderer"))]
impl BatchBudget {
    const MAX_RAW_BYTES: usize = 768 * 1024;
    const MAX_RAW_MESSAGES: usize = 16;
    const MAX_RAW_ATTEMPTS: usize = 3;

    fn from_buffer(buffer: &[QueuedMessage]) -> Self {
        Self {
            bytes: buffer.iter().map(QueuedMessage::raw_byte_len).sum(),
            attempts: buffer.iter().filter_map(QueuedMessage::attempt).collect(),
            msgs: buffer.len(),
        }
    }

    fn try_admit(
        &mut self,
        item: &QueuedMessage,
        attempt_totals: &rustc_hash::FxHashMap<u64, usize>,
    ) -> bool {
        let attempt = item.attempt();
        let admit = match attempt {
            None => {
                self.attempts.is_empty()
                    || (self.msgs < Self::MAX_RAW_MESSAGES && self.bytes < Self::MAX_RAW_BYTES)
            }
            Some(id) => {
                self.attempts.contains(&id)
                    || self.attempts.is_empty()
                    || (self.attempts.len() < Self::MAX_RAW_ATTEMPTS
                        && self.msgs < Self::MAX_RAW_MESSAGES
                        && self
                            .bytes
                            .saturating_add(attempt_totals.get(&id).copied().unwrap_or(0))
                            <= Self::MAX_RAW_BYTES)
            }
        };
        if admit {
            if let Some(id) = attempt {
                self.attempts.insert(id);
            }
            self.msgs = self.msgs.saturating_add(1);
            self.bytes = self.bytes.saturating_add(item.raw_byte_len());
        }
        admit
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
        budget: &mut BatchBudget,
        filter: &mut F,
    ) {
        let mut attempt_totals: rustc_hash::FxHashMap<u64, usize> =
            rustc_hash::FxHashMap::default();
        for item in &received.queued {
            if let Some(id) = item.attempt() {
                let entry = attempt_totals.entry(id).or_insert(0);
                *entry = entry.saturating_add(item.raw_byte_len());
            }
        }
        let mut deferred = Vec::new();
        for item in received.queued.drain(..) {
            if !budget.try_admit(&item, &attempt_totals) {
                deferred.push(item);
                continue;
            }
            if item.matches(filter) {
                ret.push(item);
            } else {
                buffer.push(item);
            }
        }
        if !deferred.is_empty() {
            self.batch_open.set(false);
        }
        received.queued = deferred;
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
                return PassingReceiveIter::new(Vec::new());
            };
            self.drain_finished(&mut received);
            if received.messages.is_empty()
                && (received.queued.is_empty()
                    || (!self.boot_unthrottled.get() && !self.batch_open.get()))
            {
                return PassingReceiveIter::new(Vec::new());
            }
        }
        let mut budget = BatchBudget::from_buffer(&buffer);
        // Partial and final tile completions must retain the worker's delivery order.
        let mut ret: Vec<QueuedMessage> = buffer
            .extract_if(.., |item| item.matches(&mut filter))
            .collect();

        let Ok(mut received) = self.received.try_borrow_mut() else {
            return PassingReceiveIter::new(ret);
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
            return PassingReceiveIter::new(ret);
        }
        if self.boot_unthrottled.get() {
            for item in received.queued.drain(..) {
                if item.matches(&mut filter) {
                    ret.push(item);
                } else {
                    buffer.push(item);
                }
            }
            return PassingReceiveIter::new(ret);
        }
        if self.batch_open.get() {
            self.admit_throttled_queued(
                &mut received,
                &mut buffer,
                &mut ret,
                &mut budget,
                &mut filter,
            );
        }
        PassingReceiveIter::new(ret)
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
        let Some(worker) = self.workers.get(index) else {
            return;
        };
        let posted = worker_message("cancel", &[("attempt", &JsValue::from(attempt))])
            .and_then(|message| worker.post_message(&message).map_err(WebError::from));
        if let Err(error) = posted {
            // The call finishes and the map ignores its result.
            tracing::warn!(attempt, %error, "cancelling a worker call failed");
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
            received.waker = None;
        }
    }
}

#[cfg(any(test, feature = "renderer"))]
mod assignments;
#[cfg(any(test, feature = "renderer"))]
mod raw_queue;
#[cfg(any(test, feature = "renderer"))]
mod recycle;
#[cfg(any(test, feature = "renderer"))]
mod style_revisions;
pub mod tag;
#[cfg(test)]
mod tests;
pub(crate) mod wire;
