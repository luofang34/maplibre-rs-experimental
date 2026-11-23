//! Wakeup, `requestAnimationFrame`, and timer scheduling for [`RunnerInner`].

#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::{cell::RefCell, rc::Rc, time::Duration};

use wasm_bindgen::{closure::Closure, JsCast, JsValue};
use web_sys::MessageChannel;

use super::{apc_has_arrivals, RunnerInner};

impl RunnerInner {
    pub(super) fn clear_wake_timer(&self) {
        self.immediate_wake_pending.set(false);
        if let Some((id, _)) = self.wake_timer.borrow_mut().take() {
            self.scope.clear_timeout_with_handle(id);
        }
    }

    pub(super) fn schedule_raf(rc: &Rc<RefCell<Self>>) {
        let should_schedule = {
            let Ok(inner) = rc.try_borrow() else {
                return;
            };
            if inner.closed.get() || inner.raf_pending.get() {
                false
            } else {
                inner.raf_pending.set(true);
                inner.clear_wake_timer();
                true
            }
        };
        if !should_schedule {
            return;
        }
        let Ok(inner) = rc.try_borrow() else {
            return;
        };
        if inner.raf_closure.borrow().is_none() {
            let weak = Rc::downgrade(rc);
            let closure = Closure::wrap(Box::new(move |_ts: f64| {
                if let Some(strong) = weak.upgrade() {
                    Self::run_frame(&strong);
                }
            }) as Box<dyn FnMut(f64)>);
            *inner.raf_closure.borrow_mut() = Some(closure);
        }
        let borrow = inner.raf_closure.borrow();
        let Some(closure) = borrow.as_ref() else {
            return;
        };
        let raf_fn =
            js_sys::Reflect::get(&inner.scope, &JsValue::from_str("requestAnimationFrame"))
                .ok()
                .and_then(|v| v.dyn_into::<js_sys::Function>().ok());
        if let Some(raf) = raf_fn {
            if let Ok(id) = raf.call1(&inner.scope, closure.as_ref().unchecked_ref()) {
                inner.raf_handle.set(id.as_f64().map(|n| n as i32));
                return;
            }
        }
        if let Ok(id) = inner
            .scope
            .set_timeout_with_callback_and_timeout_and_arguments_0(
                closure.as_ref().unchecked_ref(),
                16,
            )
        {
            inner.raf_fallback_timer.set(Some(id));
        }
    }

    pub(super) fn cancel_raf(&self) {
        self.raf_pending.set(false);
        if let Some(timer_id) = self.raf_fallback_timer.take() {
            self.scope.clear_timeout_with_handle(timer_id);
        }
        if let Some(raf_id) = self.raf_handle.take() {
            if let Ok(cancel) =
                js_sys::Reflect::get(&self.scope, &JsValue::from_str("cancelAnimationFrame"))
            {
                if let Ok(func) = cancel.dyn_into::<js_sys::Function>() {
                    drop(func.call1(&self.scope, &JsValue::from(raf_id)));
                }
            }
        }
    }

    pub(super) fn schedule_wakeup(rc: &Rc<RefCell<Self>>) {
        let pump_immediate = {
            let Ok(inner) = rc.try_borrow() else {
                return;
            };
            if inner.closed.get() || inner.immediate_wake_pending.get() {
                return;
            }
            let arrivals = apc_has_arrivals(inner.map.kernel().apc());
            if inner.raf_pending.get() {
                if !inner.initial_load_settled.get() && arrivals && inner.take_boot_pump(true) {
                    inner.cancel_raf();
                    true
                } else {
                    return;
                }
            } else if !inner.map.has_ready_work() && !arrivals && !inner.input.moved_view() {
                return;
            } else {
                inner.take_boot_pump(arrivals)
            }
        };
        if pump_immediate {
            Self::schedule_immediate_pump(rc);
        } else {
            Self::schedule_raf(rc);
        }
    }

    pub(super) fn schedule_wake(rc: &Rc<RefCell<Self>>, delay: Duration) {
        Self::schedule_timer(rc, delay, Self::schedule_raf);
    }

    pub(super) fn schedule_immediate_pump(rc: &Rc<RefCell<Self>>) {
        let should_post = {
            let Ok(inner) = rc.try_borrow() else {
                return;
            };
            if inner.closed.get() || inner.raf_pending.get() || inner.immediate_wake_pending.get() {
                return;
            }
            inner.clear_wake_timer();
            if inner.boot_wake_channel.borrow().is_none() {
                if let Ok(channel) = MessageChannel::new() {
                    let port1 = channel.port1();
                    let port2 = channel.port2();
                    let weak = Rc::downgrade(rc);
                    let closure = Closure::wrap(Box::new(move || {
                        if let Some(strong) = weak.upgrade() {
                            let still_pending = strong
                                .try_borrow()
                                .is_ok_and(|s| !s.closed.get() && s.immediate_wake_pending.get());
                            if still_pending {
                                Self::run_frame(&strong);
                            }
                        }
                    }) as Box<dyn FnMut()>);
                    port2.set_onmessage(Some(closure.as_ref().unchecked_ref()));
                    *inner.boot_wake_channel.borrow_mut() = Some((port1, port2, closure));
                }
            }
            if let Some((port1, _, _)) = inner.boot_wake_channel.borrow().as_ref() {
                if port1.post_message(&JsValue::UNDEFINED).is_ok() {
                    inner.immediate_wake_pending.set(true);
                    false
                } else {
                    true
                }
            } else {
                true
            }
        };
        if should_post {
            Self::schedule_timer(rc, Duration::ZERO, Self::run_frame);
            if let Ok(inner) = rc.try_borrow() {
                if inner.wake_timer.borrow().is_some() {
                    inner.immediate_wake_pending.set(true);
                }
            }
        }
    }

    fn schedule_timer(
        rc: &Rc<RefCell<Self>>,
        delay: Duration,
        callback_fn: fn(&Rc<RefCell<Self>>),
    ) {
        let Ok(inner) = rc.try_borrow() else {
            return;
        };
        if inner.closed.get() || inner.raf_pending.get() {
            return;
        }
        inner.clear_wake_timer();
        let weak = Rc::downgrade(rc);
        let callback = Closure::wrap(Box::new(move || {
            if let Some(strong) = weak.upgrade() {
                callback_fn(&strong);
            }
        }) as Box<dyn FnMut()>);
        let millis = i32::try_from(delay.as_millis()).unwrap_or(i32::MAX);
        if let Ok(id) = inner
            .scope
            .set_timeout_with_callback_and_timeout_and_arguments_0(
                callback.as_ref().unchecked_ref(),
                millis,
            )
        {
            *inner.wake_timer.borrow_mut() = Some((id, callback));
        }
    }
}
