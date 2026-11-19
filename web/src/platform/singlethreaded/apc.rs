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
impl<K: OffscreenKernel> AsyncProcedureCall<K> for PassingAsyncProcedureCall {
    type Context = UsedContext;
    type ReceiveIterator<F: FnMut(&Message) -> bool> = PassingReceiveIter;

    fn receive<F: FnMut(&Message) -> bool>(&self, mut filter: F) -> Self::ReceiveIterator<F> {
        const MAX_RAW_BYTES_PER_BATCH: usize = 768 * 1024;
        const MAX_RAW_MESSAGES_PER_BATCH: usize = 16;
        const MAX_RAW_ATTEMPTS_PER_BATCH: usize = 3;
        let mut buffer = self.buffer.borrow_mut();
        let mut admitted_bytes: usize = buffer.iter().map(QueuedMessage::raw_byte_len).sum();
        let mut admitted_attempts: rustc_hash::FxHashSet<u64> =
            buffer.iter().filter_map(QueuedMessage::attempt).collect();
        let mut admitted_msgs: usize = buffer.len();
        // Partial and final tile completions must retain the worker's delivery order.
        let mut ret: Vec<QueuedMessage> = buffer
            .extract_if(.., |item| item.matches(&mut filter))
            .collect();

        let Ok(mut received) = self.received.try_borrow_mut() else {
            return PassingReceiveIter::new(ret);
        };
        let mut assignments = self.assignments.borrow_mut();
        for attempt in received.finished.drain(..) {
            assignments.finished(attempt);
        }
        for message in received.messages.drain(..) {
            if filter(&message) {
                ret.push(QueuedMessage::Decoded(message));
            } else {
                buffer.push(QueuedMessage::Decoded(message));
            }
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
        if !self.batch_open.get() {
            return PassingReceiveIter::new(ret);
        }
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
            let attempt = item.attempt();
            let admit = match attempt {
                None => {
                    admitted_attempts.is_empty()
                        || (admitted_msgs < MAX_RAW_MESSAGES_PER_BATCH
                            && admitted_bytes < MAX_RAW_BYTES_PER_BATCH)
                }
                Some(id) => {
                    admitted_attempts.contains(&id)
                        || admitted_attempts.is_empty()
                        || (admitted_attempts.len() < MAX_RAW_ATTEMPTS_PER_BATCH
                            && admitted_msgs < MAX_RAW_MESSAGES_PER_BATCH
                            && admitted_bytes
                                .saturating_add(attempt_totals.get(&id).copied().unwrap_or(0))
                                <= MAX_RAW_BYTES_PER_BATCH)
                }
            };
            if !admit {
                deferred.push(item);
                continue;
            }
            if let Some(id) = attempt {
                admitted_attempts.insert(id);
            }
            admitted_msgs = admitted_msgs.saturating_add(1);
            admitted_bytes = admitted_bytes.saturating_add(item.raw_byte_len());
            if item.matches(&mut filter) {
                ret.push(item);
            } else {
                buffer.push(item);
            }
        }
        if !deferred.is_empty() {
            self.batch_open.set(false);
        }
        received.queued = deferred;

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
