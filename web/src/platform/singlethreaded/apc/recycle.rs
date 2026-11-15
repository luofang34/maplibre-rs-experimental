//! Idle worker pool recycling for single-threaded (`wasm32` without atomics) maps.

use std::{cell::RefCell, rc::Rc};

use js_sys::{Array, ArrayBuffer};
use wasm_bindgen::{closure::Closure, JsCast, JsValue};
use web_sys::{MessageEvent, Worker};

use super::{worker_message, ReceivedType};
use crate::{error::WebError, platform::singlethreaded::wasm_entries::deliver_received_data};

const MAX_RECYCLED_ST_WORKERS: usize = 4;

thread_local! {
    static RECYCLED_ST_WORKERS: RefCell<Vec<Worker>> = const { RefCell::new(Vec::new()) };
}

pub(super) type WorkerMessageClosure = Closure<dyn FnMut(MessageEvent)>;

pub(super) fn acquire_workers(
    new_worker: &js_sys::Function,
    initial_workers: usize,
    config_json: &str,
    received: &Rc<ReceivedType>,
) -> Result<(Vec<Worker>, Vec<WorkerMessageClosure>), WebError> {
    let mut workers = Vec::with_capacity(initial_workers);
    let mut listeners = Vec::with_capacity(initial_workers);
    let configure = worker_message("kernel_config", &[("config", &JsValue::from(config_json))])?;
    let mut init_msg: Option<js_sys::Object> = None;

    for _ in 0..initial_workers {
        let (worker, is_recycled) =
            if let Some(reused) = RECYCLED_ST_WORKERS.with(|bin| bin.borrow_mut().pop()) {
                (reused, true)
            } else {
                let created = new_worker
                    .call1(&JsValue::undefined(), &JsValue::from(0_u32))
                    .map_err(WebError::from)?
                    .dyn_into::<Worker>()
                    .map_err(|_| WebError::TypeError("Unable to cast to Worker".into()))?;
                (created, false)
            };

        let listener = make_worker_listener(received);
        worker.set_onmessage(Some(listener.as_ref().unchecked_ref()));

        if !is_recycled {
            let init = match &init_msg {
                Some(msg) => msg,
                None => init_msg.insert(worker_message(
                    "wasm_init",
                    &[("module", &wasm_bindgen::module())],
                )?),
            };
            worker.post_message(init).map_err(WebError::from)?;
        }
        worker.post_message(&configure).map_err(WebError::from)?;
        workers.push(worker);
        listeners.push(listener);
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

pub(super) fn release_workers(
    workers: Vec<Worker>,
    mut listeners: Vec<WorkerMessageClosure>,
    busy_indices: &[bool],
) {
    listeners.clear();
    RECYCLED_ST_WORKERS.with(|bin| {
        let mut bin = bin.borrow_mut();
        for (idx, worker) in workers.into_iter().enumerate() {
            worker.set_onmessage(None);
            let is_busy = busy_indices.get(idx).copied().unwrap_or(false);
            if !is_busy && bin.len() < MAX_RECYCLED_ST_WORKERS {
                bin.push(worker);
            } else {
                worker.terminate();
            }
        }
    });
}
