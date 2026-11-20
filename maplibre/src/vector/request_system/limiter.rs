//! Bounds concurrent expanded `geozero::mvt::Tile` AST and `SymbolAtlas` materialisations on
//! `wasm32`, and bounds decoded/unconsumed tiles in shared-memory channels (`wasm32 + atomics`)
//! while allowing raw network `fetch` requests and asset prefetches to run with full concurrency.

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    future::poll_fn,
    sync::{LazyLock, Mutex},
    task::{Poll, Waker},
};

#[cfg(any(test, all(target_arch = "wasm32", target_feature = "atomics")))]
const MAX_HEAVY_TILES_LOW_MEM: usize = 4;
#[cfg(any(test, all(target_arch = "wasm32", target_feature = "atomics")))]
const MAX_HEAVY_TILES_HIGH_MEM: usize = 2;
/// 40 MiB (`640 * 64 KiB` WASM pages): run all 4 workers in parallel during low-memory boot and
/// light scenes, then throttle heavy decodes to 2 once shared linear memory reaches 40 MiB so
/// dense flight bursts reuse freed pages instead of permanently growing `WebAssembly.Memory`.
#[cfg(any(test, all(target_arch = "wasm32", target_feature = "atomics")))]
const HEAVY_TILE_THROTTLE_PAGES: usize = 40 * 16;

#[cfg(any(test, all(target_arch = "wasm32", target_feature = "atomics")))]
const fn heavy_tile_limit_for_pages(pages: usize) -> usize {
    if pages >= HEAVY_TILE_THROTTLE_PAGES {
        MAX_HEAVY_TILES_HIGH_MEM
    } else {
        MAX_HEAVY_TILES_LOW_MEM
    }
}

fn current_heavy_tile_limit() -> usize {
    #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
    {
        heavy_tile_limit_for_pages(core::arch::wasm32::memory_size::<0>())
    }
    #[cfg(not(all(target_arch = "wasm32", target_feature = "atomics")))]
    {
        1
    }
}

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
const MAX_UNCONSUMED_SHARED_TILES: usize = 4;

#[derive(Default)]
struct PermitState {
    in_use: usize,
    waiters: Vec<Waker>,
}

impl PermitState {
    fn poll_acquire(&mut self, limit: usize, waker: &Waker) -> Poll<()> {
        if self.in_use < limit {
            self.in_use += 1;
            Poll::Ready(())
        } else {
            if !self.waiters.iter().any(|w| w.will_wake(waker)) {
                self.waiters.push(waker.clone());
            }
            Poll::Pending
        }
    }

    fn release(&mut self) -> Vec<Waker> {
        self.in_use = self.in_use.saturating_sub(1);
        std::mem::take(&mut self.waiters)
    }
}

static LIMITER: LazyLock<Mutex<PermitState>> = LazyLock::new(|| Mutex::new(PermitState::default()));

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
static SHARED_TILES: LazyLock<Mutex<PermitState>> =
    LazyLock::new(|| Mutex::new(PermitState::default()));

async fn acquire_slot(gate: &'static Mutex<PermitState>, mut limit: impl FnMut() -> usize) {
    poll_fn(|cx| crate::platform::lock_unpoisoned(gate).poll_acquire(limit(), cx.waker())).await;
}

fn release_slot(gate: &'static Mutex<PermitState>) {
    let waiters = crate::platform::lock_unpoisoned(gate).release();
    for waker in waiters {
        waker.wake();
    }
}

/// RAII token bounding concurrent heavy CPU decodes / tessellations.
pub(crate) struct HeavyTileSlot {
    _private: (),
}

impl Drop for HeavyTileSlot {
    fn drop(&mut self) {
        release_slot(&LIMITER);
    }
}

/// Acquires one heavy-decode slot, waiting cleanly until `current_heavy_tile_limit()` allows
/// admission.
pub(crate) async fn acquire_heavy_tile_slot() -> HeavyTileSlot {
    acquire_slot(&LIMITER, current_heavy_tile_limit).await;
    HeavyTileSlot { _private: () }
}

/// RAII token reserved after HTTP fetch and released when the completed tile message is consumed.
#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
#[derive(Debug)]
pub(crate) struct SharedTilePermit {
    _private: (),
}

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
impl Drop for SharedTilePermit {
    fn drop(&mut self) {
        release_slot(&SHARED_TILES);
    }
}

/// Reserves one unconsumed shared-memory tile slot upfront before decode starts.
#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
pub(crate) async fn acquire_shared_tile_permit() -> SharedTilePermit {
    acquire_slot(&SHARED_TILES, || MAX_UNCONSUMED_SHARED_TILES).await;
    SharedTilePermit { _private: () }
}

/// Shared-memory tile permit holder that compiles away on non-atomics targets.
#[derive(Debug, Default)]
pub(crate) struct SharedTileSlot {
    #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
    permit: Option<SharedTilePermit>,
}

impl SharedTileSlot {
    /// Transfers any held permit from `other` into `self` when `self` has not claimed one yet.
    pub(crate) fn merge(&mut self, other: Self) {
        #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
        if self.permit.is_none() {
            self.permit = other.permit;
        }
        #[cfg(not(all(target_arch = "wasm32", target_feature = "atomics")))]
        let _ = (self, other);
    }

    /// Attaches the permit to an APC outcome message so it drops when the main thread consumes it.
    pub(crate) fn attach(self, message: crate::io::apc::Message) -> crate::io::apc::Message {
        #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
        {
            match self.permit {
                Some(permit) => message.with_shared_permit(permit),
                None => message,
            }
        }
        #[cfg(not(all(target_arch = "wasm32", target_feature = "atomics")))]
        message
    }
}

/// Ensures at most one concurrent source within a tile request claims a [`SharedTileSlot`].
#[derive(Default)]
pub(crate) struct SharedTileClaim {
    #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
    claimed: AtomicBool,
}

impl SharedTileClaim {
    /// Claims a [`SharedTileSlot`] the first time this helper is awaited for a tile request.
    pub(crate) async fn acquire_once(&self) -> SharedTileSlot {
        #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
        {
            if !self.claimed.swap(true, Ordering::AcqRel) {
                return SharedTileSlot {
                    permit: Some(acquire_shared_tile_permit().await),
                };
            }
        }
        SharedTileSlot::default()
    }
}

#[cfg(any(test, all(target_arch = "wasm32", target_feature = "atomics")))]
struct PermitQueue<T>(Vec<(crate::coords::WorldTileCoords, T)>);

#[cfg(any(test, all(target_arch = "wasm32", target_feature = "atomics")))]
impl<T> Default for PermitQueue<T> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

#[cfg(any(test, all(target_arch = "wasm32", target_feature = "atomics")))]
impl<T> PermitQueue<T> {
    fn push(&mut self, coords: crate::coords::WorldTileCoords, permit: T) {
        self.0.retain(|(existing, _)| *existing != coords);
        self.0.push((coords, permit));
    }

    fn retain_deferred(
        &mut self,
        any_deferred: bool,
        mut keep: impl FnMut(crate::coords::WorldTileCoords) -> bool,
    ) {
        if !any_deferred {
            self.0.clear();
            return;
        }
        self.0.retain(|(coords, _)| keep(*coords));
    }

    fn clear(&mut self) {
        self.0.clear();
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Holds `SharedTilePermit`s for decoded tiles queued in `world.tiles` until `upload_system`
/// finishes uploading them (or clears immediately when no upload is budget-deferred).
#[derive(Default)]
pub(crate) struct PendingUploadPermits {
    #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
    inner: PermitQueue<SharedTilePermit>,
}

impl PendingUploadPermits {
    #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
    pub(crate) fn push(
        &mut self,
        coords: crate::coords::WorldTileCoords,
        permit: SharedTilePermit,
    ) {
        self.inner.push(coords, permit);
    }

    pub(crate) fn retain_deferred(
        &mut self,
        any_deferred: bool,
        mut keep: impl FnMut(crate::coords::WorldTileCoords) -> bool,
    ) {
        #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
        self.inner.retain_deferred(any_deferred, &mut keep);
        #[cfg(not(all(target_arch = "wasm32", target_feature = "atomics")))]
        let _ = (any_deferred, &mut keep);
    }

    pub(crate) fn clear(&mut self) {
        #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
        self.inner.clear();
    }

    pub(crate) fn is_empty(&self) -> bool {
        #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
        {
            self.inner.is_empty()
        }
        #[cfg(not(all(target_arch = "wasm32", target_feature = "atomics")))]
        {
            true
        }
    }
}

pub(crate) fn clear_upload_permits(world: &mut crate::tcs::world::World) {
    if let Some(permits) = world.resources.get_mut::<PendingUploadPermits>() {
        permits.clear();
    }
}

pub(crate) fn snapshot_permit_sources(
    world: &crate::tcs::world::World,
    active: bool,
    source_tiles: &[crate::coords::WorldTileCoords],
) -> Option<rustc_hash::FxHashSet<crate::coords::WorldTileCoords>> {
    (active
        && world
            .resources
            .get::<PendingUploadPermits>()
            .is_some_and(|permits| !permits.is_empty()))
    .then(|| source_tiles.iter().copied().collect())
}

pub(crate) fn reconcile_upload_permits(
    world: &mut crate::tcs::world::World,
    deferred: bool,
    style_revision: u64,
    permit_source_set: Option<&rustc_hash::FxHashSet<crate::coords::WorldTileCoords>>,
) {
    use crate::{
        render::eventually::Eventually::Initialized,
        vector::{content::LayerReplacements, VectorBufferPool, VectorLayerBucketComponent},
    };
    let Some(mut permits) = world.resources.remove::<PendingUploadPermits>() else {
        return;
    };
    let end_stamp = deferred.then(|| {
        world
            .resources
            .get::<crate::render::eventually::Eventually<VectorBufferPool>>()
            .and_then(|pool| match pool {
                Initialized(pool) => Some((style_revision, pool.eviction_generation())),
                _ => None,
            })
    });
    permits.retain_deferred(deferred && permit_source_set.is_some(), |coords| {
        permit_source_set.is_some_and(|set| set.contains(&coords))
            && world
                .tiles
                .query::<(&VectorLayerBucketComponent, &LayerReplacements)>(coords)
                .is_some_and(|(layers, pending)| {
                    layers.done
                        && (!pending.vector.is_empty()
                            || end_stamp
                                .flatten()
                                .is_some_and(|stamp| layers.uploaded_stamp != Some(stamp)))
                })
    });
    world.resources.insert(permits);
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        task::Wake,
    };

    use super::*;

    struct CountWake(AtomicUsize);

    impl Wake for CountWake {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn permit_state_reserves_upfront_blocks_at_limit_and_wakes_waiter_on_release() {
        let counter = Arc::new(CountWake(AtomicUsize::new(0)));
        let waker = Waker::from(counter.clone());
        let mut state = PermitState::default();

        assert!(state.poll_acquire(2, &waker).is_ready());
        assert!(state.poll_acquire(2, &waker).is_ready());
        assert_eq!(state.in_use, 2);

        // 3rd waiter blocks at limit and deduplicates identical wakers.
        assert!(state.poll_acquire(2, &waker).is_pending());
        assert!(state.poll_acquire(2, &waker).is_pending());
        assert_eq!(state.waiters.len(), 1);

        for w in state.release() {
            w.wake();
        }
        assert_eq!(state.in_use, 1);
        assert_eq!(counter.0.load(Ordering::Relaxed), 1);
        assert!(state.poll_acquire(2, &waker).is_ready());
        assert_eq!(state.in_use, 2);
    }

    #[test]
    fn pending_upload_permits_deduplicates_and_releases_when_not_deferred() {
        use crate::coords::WorldTileCoords;

        let t0 = WorldTileCoords {
            x: 1,
            y: 2,
            z: 14.into(),
        };
        let t1 = WorldTileCoords {
            x: 2,
            y: 2,
            z: 14.into(),
        };
        let mut queue = PermitQueue::default();
        assert!(queue.is_empty());

        queue.push(t0, "first");
        queue.push(t0, "replaced");
        queue.push(t1, "second");
        assert_eq!(queue.0, vec![(t0, "replaced"), (t1, "second")]);

        // While upload budget deferred tiles, only visible still-pending coords remain.
        queue.retain_deferred(true, |coords| coords == t1);
        assert_eq!(queue.0, vec![(t1, "second")]);

        // As soon as a frame ends without deferral, all permits release to prevent deadlock.
        queue.retain_deferred(false, |_| true);
        assert!(queue.is_empty());

        queue.push(t0, "readded");
        queue.clear();
        assert!(queue.is_empty());
    }

    #[test]
    fn heavy_tile_limit_throttles_above_threshold_and_drains_cleanly() {
        assert_eq!(heavy_tile_limit_for_pages(0), 4);
        assert_eq!(heavy_tile_limit_for_pages(639), 4);
        assert_eq!(heavy_tile_limit_for_pages(640), 2);
        assert_eq!(heavy_tile_limit_for_pages(1024), 2);

        let counter = Arc::new(CountWake(AtomicUsize::new(0)));
        let waker = Waker::from(counter.clone());
        let mut state = PermitState::default();
        for _ in 0..4 {
            assert!(state.poll_acquire(4, &waker).is_ready());
        }
        assert_eq!(state.in_use, 4);

        // Tightening limit mid-flight blocks new work until in-flight count drains below 2.
        assert!(state
            .poll_acquire(heavy_tile_limit_for_pages(640), &waker)
            .is_pending());
        for w in state.release() {
            w.wake();
        }
        assert_eq!(state.in_use, 3);
        assert!(state
            .poll_acquire(heavy_tile_limit_for_pages(640), &waker)
            .is_pending());
        for w in state.release() {
            w.wake();
        }
        assert_eq!(state.in_use, 2);
        assert!(state
            .poll_acquire(heavy_tile_limit_for_pages(640), &waker)
            .is_pending());
        for w in state.release() {
            w.wake();
        }
        assert_eq!(state.in_use, 1);
        assert!(state
            .poll_acquire(heavy_tile_limit_for_pages(640), &waker)
            .is_ready());
        assert_eq!(state.in_use, 2);
    }
}
