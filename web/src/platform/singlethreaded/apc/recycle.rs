//! Idle worker pool recycling for single-threaded (`wasm32` without atomics) maps.

use std::{cell::RefCell, rc::Rc};

use js_sys::{Array, ArrayBuffer};
use wasm_bindgen::{closure::Closure, JsCast, JsValue};
use web_sys::{MessageEvent, MessagePort, Worker};

use super::{deliver_received_data, ReceivedType};
use crate::error::WebError;

/// Builds a worker message `{ type: kind, <key>: value, ... }`.
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

const MAX_RECYCLED_ST_WORKERS: usize = 6;

pub(super) enum WorkerEndpoint {
    Dedicated(Worker),
    Port(MessagePort),
}

impl WorkerEndpoint {
    pub(super) fn as_js_value(&self) -> &JsValue {
        match self {
            Self::Dedicated(worker) => worker.as_ref(),
            Self::Port(port) => port.as_ref(),
        }
    }

    pub(super) fn post_message(&self, msg: &JsValue) -> Result<(), JsValue> {
        match self {
            Self::Dedicated(worker) => worker.post_message(msg),
            Self::Port(port) => port.post_message(msg),
        }
    }

    fn set_onmessage(&self, callback: Option<&js_sys::Function>) {
        match self {
            Self::Dedicated(worker) => worker.set_onmessage(callback),
            Self::Port(port) => {
                port.set_onmessage(callback);
                if callback.is_some() {
                    port.start();
                }
            }
        }
    }
}

thread_local! {
    static RECYCLED_ST_WORKERS: RefCell<Vec<Worker>> = const { RefCell::new(Vec::new()) };
}

pub(super) type WorkerMessageClosure = Closure<dyn FnMut(MessageEvent)>;

pub(super) fn acquire_workers(
    new_worker: &js_sys::Function,
    initial_workers: usize,
    config_json: &str,
    received: &Rc<ReceivedType>,
) -> Result<(Vec<WorkerEndpoint>, Vec<WorkerMessageClosure>), WebError> {
    let mut workers = Vec::with_capacity(initial_workers);
    let mut listeners = Vec::with_capacity(initial_workers);
    let configure = worker_message("kernel_config", &[("config", &JsValue::from(config_json))])?;
    let mut init_msg: Option<js_sys::Object> = None;

    for _ in 0..initial_workers {
        let (endpoint, needs_wasm_init) =
            if let Some(reused) = RECYCLED_ST_WORKERS.with(|bin| bin.borrow_mut().pop()) {
                (WorkerEndpoint::Dedicated(reused), false)
            } else {
                let created = new_worker
                    .call1(&JsValue::undefined(), &JsValue::from(0_u32))
                    .map_err(WebError::from)?;
                if created.is_undefined() || created.is_null() {
                    break;
                }
                match created.dyn_into::<Worker>() {
                    Ok(worker) => (WorkerEndpoint::Dedicated(worker), true),
                    Err(other) => {
                        let port = other.dyn_into::<MessagePort>().map_err(|_| {
                            WebError::TypeError(
                                "Unable to cast worker endpoint to Worker or MessagePort".into(),
                            )
                        })?;
                        // Pre-initialized MessagePort endpoints receive `wasm_init` from the host.
                        (WorkerEndpoint::Port(port), false)
                    }
                }
            };

        let listener = make_worker_listener(received);
        endpoint.set_onmessage(Some(listener.as_ref().unchecked_ref()));

        if needs_wasm_init {
            let init = match &init_msg {
                Some(msg) => msg,
                None => init_msg.insert(worker_message(
                    "wasm_init",
                    &[("module", &wasm_bindgen::module())],
                )?),
            };
            endpoint.post_message(init).map_err(WebError::from)?;
        }
        endpoint.post_message(&configure).map_err(WebError::from)?;
        workers.push(endpoint);
        listeners.push(listener);
    }
    if workers.is_empty() {
        return Err(WebError::TypeError(
            "Unable to create any single-threaded tile workers".into(),
        ));
    }

    Ok((workers, listeners))
}

fn make_worker_listener(received: &Rc<ReceivedType>) -> WorkerMessageClosure {
    let weak = Rc::downgrade(received);
    Closure::wrap(Box::new(move |event: MessageEvent| {
        let Some(received) = weak.upgrade() else {
            return;
        };
        let Ok(tuple) = event.data().dyn_into::<Array>() else {
            return;
        };
        let Some(tag_f64) = tuple.get(0).as_f64() else {
            return;
        };
        let Ok(buffer) = tuple.get(1).dyn_into::<ArrayBuffer>() else {
            return;
        };
        if let Err(err) = deliver_received_data(&received, tag_f64 as u32, buffer) {
            tracing::warn!(?err, "failed delivering single-threaded worker message");
        }
    }) as Box<dyn FnMut(MessageEvent)>)
}

pub(super) fn release_workers_with_busy(
    workers: Vec<WorkerEndpoint>,
    listeners: Vec<WorkerMessageClosure>,
    busy_worker_indices: impl Iterator<Item = usize>,
) {
    let mut busy = vec![false; workers.len()];
    for idx in busy_worker_indices {
        if let Some(slot) = busy.get_mut(idx) {
            *slot = true;
        }
    }
    release_workers(workers, listeners, &busy);
}

pub(super) fn release_workers(
    workers: Vec<WorkerEndpoint>,
    mut listeners: Vec<WorkerMessageClosure>,
    busy_indices: &[bool],
) {
    for endpoint in &workers {
        endpoint.set_onmessage(None);
    }
    listeners.clear();
    RECYCLED_ST_WORKERS.with(|bin| {
        let mut bin = bin.borrow_mut();
        for (idx, endpoint) in workers.into_iter().enumerate() {
            match endpoint {
                WorkerEndpoint::Dedicated(worker) => {
                    let is_busy = busy_indices.get(idx).copied().unwrap_or(false);
                    if !is_busy && bin.len() < MAX_RECYCLED_ST_WORKERS {
                        bin.push(worker);
                    } else {
                        worker.terminate();
                    }
                }
                WorkerEndpoint::Port(port) => {
                    port.close();
                }
            }
        }
    });
}
