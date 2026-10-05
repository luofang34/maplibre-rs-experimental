//! Bounds how many generators run at once and how many requests may wait for one.

use std::{
    future::poll_fn,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    task::{Poll, Waker},
};

#[derive(Debug, Default)]
struct State {
    running: usize,
    waiting: usize,
    wakers: Vec<Waker>,
}

/// Running and waiting generator slots, shared by every clone of a provider registry.
#[derive(Debug, Default)]
pub(super) struct Gate {
    state: Mutex<State>,
}

/// The waiting queue is full; the request is refused rather than queued without bound.
#[derive(Debug)]
pub(super) struct QueueFull;

/// A running slot, given back when dropped.
pub(super) struct Permit {
    gate: Arc<Gate>,
}

/// A place in the waiting queue, given back when dropped, whether a slot was won or the
/// request was cancelled.
struct Waiting {
    gate: Arc<Gate>,
}

impl Gate {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// How many generators run and how many requests wait for one.
    pub(super) fn load(&self) -> (usize, usize) {
        let state = self.lock();
        (state.running, state.waiting)
    }

    /// Takes a running slot, waiting in line while `running` are busy, unless `waiting`
    /// requests already wait.
    pub(super) async fn enter(
        gate: &Arc<Gate>,
        running: usize,
        waiting: usize,
    ) -> Result<Permit, QueueFull> {
        {
            let mut state = gate.lock();
            if state.running < running {
                state.running += 1;
                return Ok(Permit { gate: gate.clone() });
            }
            if state.waiting >= waiting {
                return Err(QueueFull);
            }
            state.waiting += 1;
        }
        let place = Waiting { gate: gate.clone() };
        poll_fn(|context| {
            let mut state = gate.lock();
            if state.running < running {
                state.running += 1;
                Poll::Ready(())
            } else {
                state.wakers.push(context.waker().clone());
                Poll::Pending
            }
        })
        .await;
        drop(place);
        Ok(Permit { gate: gate.clone() })
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        // Every waiter checks again; the ones that lose go back to waiting.
        let wakers = {
            let mut state = self.gate.lock();
            state.running = state.running.saturating_sub(1);
            std::mem::take(&mut state.wakers)
        };
        wakers.into_iter().for_each(Waker::wake);
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        let mut state = self.gate.lock();
        state.waiting = state.waiting.saturating_sub(1);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
