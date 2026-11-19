//! Bounds concurrent expanded `geozero::mvt::Tile` AST and `SymbolAtlas` materialisations on
//! `wasm32`, and bounds unconsumed completed tiles in shared-memory channels (`wasm32 + atomics`)
//! while allowing raw network `fetch` requests and asset prefetches to run with full concurrency.

use std::{
    future::poll_fn,
    sync::{LazyLock, Mutex},
    task::{Poll, Waker},
};

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
const MAX_HEAVY_TILES: usize = 2;
#[cfg(all(target_arch = "wasm32", not(target_feature = "atomics")))]
const MAX_HEAVY_TILES: usize = 1;
#[cfg(not(target_arch = "wasm32"))]
const MAX_HEAVY_TILES: usize = 1;

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
const MAX_UNCONSUMED_SHARED_TILES: usize = 8;

#[derive(Default)]
struct LimiterState {
    in_use: usize,
    waiters: Vec<Waker>,
}

static LIMITER: LazyLock<Mutex<LimiterState>> =
    LazyLock::new(|| Mutex::new(LimiterState::default()));

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
#[derive(Default)]
struct SharedTileState {
    unconsumed: usize,
    waiters: Vec<Waker>,
}

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
static SHARED_TILES: LazyLock<Mutex<SharedTileState>> =
    LazyLock::new(|| Mutex::new(SharedTileState::default()));

pub(super) struct HeavyTileSlot {
    _private: (),
}

impl Drop for HeavyTileSlot {
    fn drop(&mut self) {
        let waiters = {
            let mut state = crate::platform::lock_unpoisoned(&LIMITER);
            state.in_use = state.in_use.saturating_sub(1);
            std::mem::take(&mut state.waiters)
        };
        for waker in waiters {
            waker.wake();
        }
    }
}

pub(super) async fn acquire_heavy_tile_slot() -> HeavyTileSlot {
    poll_fn(|cx| {
        let mut state = crate::platform::lock_unpoisoned(&LIMITER);
        if state.in_use < MAX_HEAVY_TILES {
            state.in_use += 1;
            Poll::Ready(HeavyTileSlot { _private: () })
        } else {
            if !state.waiters.iter().any(|w| w.will_wake(cx.waker())) {
                state.waiters.push(cx.waker().clone());
            }
            Poll::Pending
        }
    })
    .await
}

/// RAII token attached to a completed `wasm32 + atomics` vector tile outcome so the worker yields
/// when `MAX_UNCONSUMED_SHARED_TILES` completed tiles are still awaiting main-thread consumption.
#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
#[derive(Debug)]
pub(crate) struct SharedTilePermit {
    _private: (),
}

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
impl Drop for SharedTilePermit {
    fn drop(&mut self) {
        let waiters = {
            let mut state = crate::platform::lock_unpoisoned(&SHARED_TILES);
            state.unconsumed = state.unconsumed.saturating_sub(1);
            std::mem::take(&mut state.waiters)
        };
        for waker in waiters {
            waker.wake();
        }
    }
}

pub(super) async fn wait_for_shared_tile_slot() {
    #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
    {
        poll_fn(|cx| {
            let mut state = crate::platform::lock_unpoisoned(&SHARED_TILES);
            if state.unconsumed < MAX_UNCONSUMED_SHARED_TILES {
                Poll::Ready(())
            } else {
                if !state.waiters.iter().any(|w| w.will_wake(cx.waker())) {
                    state.waiters.push(cx.waker().clone());
                }
                Poll::Pending
            }
        })
        .await;
    }
}

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
pub(super) fn issue_shared_tile_permit() -> SharedTilePermit {
    let mut state = crate::platform::lock_unpoisoned(&SHARED_TILES);
    state.unconsumed += 1;
    SharedTilePermit { _private: () }
}
