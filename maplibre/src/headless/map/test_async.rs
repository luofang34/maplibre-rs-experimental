//! Runtime-agnostic async coordination helpers for `headless::map` unit tests.
#![allow(dead_code, clippy::expect_used)]

use std::{
    future::{poll_fn, Future},
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    task::{Context, Poll, Waker},
    time::Duration,
};

/// Drives an async test body on the platform's executor without coupling test modules to an
/// executor crate directly.
pub(super) fn run_async<F: Future>(future: F) -> F::Output {
    #[cfg(all(not(target_arch = "wasm32"), feature = "native-io"))]
    {
        crate::platform::run_multithreaded(future)
    }
    #[cfg(not(all(not(target_arch = "wasm32"), feature = "native-io")))]
    {
        futures::executor::block_on(future)
    }
}

/// Yields control for one poll cycle using `std::future::poll_fn`.
pub(super) async fn yield_once() {
    let mut yielded = false;
    poll_fn(|cx| {
        if yielded {
            Poll::Ready(())
        } else {
            yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    })
    .await;
}

#[derive(Default)]
struct NotifyInner {
    permits: usize,
    epoch: u64,
    wakers: Vec<Waker>,
}

/// Runtime-agnostic async notification primitive supporting single-waiter permits and broadcasts.
#[derive(Default)]
pub(super) struct Notify {
    inner: Mutex<NotifyInner>,
}

impl Notify {
    pub(super) fn notify_one(&self) {
        let waker = {
            let mut inner = self.inner.lock().expect("notify lock");
            if inner.wakers.is_empty() {
                inner.permits = 1;
                None
            } else {
                inner.permits = inner.permits.saturating_add(1);
                inner.wakers.pop()
            }
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    pub(super) fn notify_waiters(&self) {
        let wakers = {
            let mut inner = self.inner.lock().expect("notify lock");
            inner.epoch = inner.epoch.wrapping_add(1);
            std::mem::take(&mut inner.wakers)
        };
        for waker in wakers {
            waker.wake();
        }
    }

    pub(super) fn notified(&self) -> Notified<'_> {
        let seen_epoch = self.inner.lock().expect("notify lock").epoch;
        Notified {
            notify: self,
            seen_epoch,
        }
    }
}

pub(super) struct Notified<'a> {
    notify: &'a Notify,
    seen_epoch: u64,
}

impl Future for Notified<'_> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut inner = self.notify.inner.lock().expect("notify lock");
        if inner.epoch != self.seen_epoch {
            return Poll::Ready(());
        }
        if inner.permits > 0 {
            inner.permits -= 1;
            return Poll::Ready(());
        }
        if !inner.wakers.iter().any(|w| w.will_wake(cx.waker())) {
            inner.wakers.push(cx.waker().clone());
        }
        Poll::Pending
    }
}

struct GateInner {
    permits: usize,
    closed: bool,
    wakers: Vec<Waker>,
}

/// Runtime-agnostic permit gate for holding and releasing async asset fetches in tests.
pub(super) struct PermitGate {
    inner: Mutex<GateInner>,
}

impl PermitGate {
    pub(super) fn new(permits: usize) -> Self {
        Self {
            inner: Mutex::new(GateInner {
                permits,
                closed: false,
                wakers: Vec::new(),
            }),
        }
    }

    pub(super) async fn acquire(&self) -> Result<(), ()> {
        poll_fn(|cx| {
            let mut inner = self.inner.lock().expect("gate lock");
            if inner.closed {
                return Poll::Ready(Err(()));
            }
            if inner.permits > 0 {
                inner.permits -= 1;
                return Poll::Ready(Ok(()));
            }
            if !inner.wakers.iter().any(|w| w.will_wake(cx.waker())) {
                inner.wakers.push(cx.waker().clone());
            }
            Poll::Pending
        })
        .await
    }

    pub(super) fn add_permits(&self, count: usize) {
        let wakers = {
            let mut inner = self.inner.lock().expect("gate lock");
            inner.permits = inner.permits.saturating_add(count);
            std::mem::take(&mut inner.wakers)
        };
        for waker in wakers {
            waker.wake();
        }
    }

    pub(super) fn close(&self) {
        let wakers = {
            let mut inner = self.inner.lock().expect("gate lock");
            inner.closed = true;
            std::mem::take(&mut inner.wakers)
        };
        for waker in wakers {
            waker.wake();
        }
    }
}

struct SleepTimer {
    cancel: Arc<(Mutex<bool>, Condvar)>,
    done: Arc<AtomicBool>,
    waker: Arc<Mutex<Option<Waker>>>,
    duration: Duration,
    started: bool,
}

impl Future for SleepTimer {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.done.load(Ordering::Acquire) {
            return Poll::Ready(());
        }
        *self.waker.lock().expect("timer waker") = Some(cx.waker().clone());
        if !self.started {
            self.started = true;
            let cancel = Arc::clone(&self.cancel);
            let done = Arc::clone(&self.done);
            let waker = Arc::clone(&self.waker);
            let duration = self.duration;
            std::thread::spawn(move || {
                let (lock, cvar) = &*cancel;
                let guard = lock.lock().expect("timer cancel");
                let (guard, _) = cvar
                    .wait_timeout_while(guard, duration, |cancelled| !*cancelled)
                    .expect("timer wait");
                if !*guard {
                    done.store(true, Ordering::Release);
                    if let Some(waker) = waker.lock().expect("timer waker").take() {
                        waker.wake();
                    }
                }
            });
        }
        if self.done.load(Ordering::Acquire) {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

impl Drop for SleepTimer {
    fn drop(&mut self) {
        if self.started && !self.done.load(Ordering::Acquire) {
            let (lock, cvar) = &*self.cancel;
            *lock.lock().expect("timer cancel") = true;
            cvar.notify_one();
        }
    }
}

pub(super) async fn sleep(duration: Duration) {
    SleepTimer {
        cancel: Arc::new((Mutex::new(false), Condvar::new())),
        done: Arc::new(AtomicBool::new(false)),
        waker: Arc::new(Mutex::new(None)),
        duration,
        started: false,
    }
    .await;
}

pub(super) async fn timeout<F: Future>(duration: Duration, future: F) -> Result<F::Output, ()> {
    let future = std::pin::pin!(future);
    let timer = std::pin::pin!(sleep(duration));
    match futures::future::select(future, timer).await {
        futures::future::Either::Left((output, _)) => Ok(output),
        futures::future::Either::Right(((), _)) => Err(()),
    }
}
