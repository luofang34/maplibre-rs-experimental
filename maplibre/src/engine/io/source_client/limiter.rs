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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MtMemoryPolicy {
    throttle_mib: usize,
    heavy_low_mem: usize,
    heavy_high_mem: usize,
    dem_low_mem: usize,
    dem_high_mem: usize,
    shared_low_mem: usize,
    shared_high_mem: usize,
}

#[cfg(any(test, all(target_arch = "wasm32", target_feature = "atomics")))]
impl MtMemoryPolicy {
    const WASM_PAGES_PER_MIB: usize = 16;

    /// Conservative baseline (`24 MiB`, `384 * 64 KiB` pages) suited for `<= 2 GiB` devices.
    const CONSERVATIVE_24_MIB: Self = Self {
        throttle_mib: 24,
        heavy_low_mem: 4,
        heavy_high_mem: 2,
        dem_low_mem: 2,
        dem_high_mem: 1,
        shared_low_mem: 8,
        shared_high_mem: 4,
    };

    /// Balanced tier (`40 MiB`, `640 * 64 KiB` pages) suited for `<= 4 GiB` mid-memory devices.
    const BALANCED_40_MIB: Self = Self {
        throttle_mib: 40,
        heavy_low_mem: 4,
        heavy_high_mem: 2,
        dem_low_mem: 3,
        dem_high_mem: 1,
        shared_low_mem: 8,
        shared_high_mem: 4,
    };

    /// Standard tier (`56 MiB`, `896 * 64 KiB` pages): allows 4 parallel heavy/DEM workers during
    /// boot and scene transitions before tightening to 2 once shared memory crosses 56 MiB.
    const STANDARD_56_MIB: Self = Self {
        throttle_mib: 56,
        heavy_low_mem: 4,
        heavy_high_mem: 2,
        dem_low_mem: 4,
        dem_high_mem: 2,
        shared_low_mem: 8,
        shared_high_mem: 4,
    };

    /// Selects the multithreaded decode memory tier for an optional `navigator.deviceMemory` GiB
    /// hint (`None` defaults to `STANDARD_56_MIB`).
    const fn for_device_memory_gib(device_memory_gib: Option<u32>) -> Self {
        match device_memory_gib {
            Some(gib) if gib <= 2 => Self::CONSERVATIVE_24_MIB,
            Some(gib) if gib <= 4 => Self::BALANCED_40_MIB,
            Some(_) | None => Self::STANDARD_56_MIB,
        }
    }

    const fn throttle_pages(self) -> usize {
        self.throttle_mib * Self::WASM_PAGES_PER_MIB
    }

    const fn heavy_limit(self, pages: usize) -> usize {
        if pages >= self.throttle_pages() {
            self.heavy_high_mem
        } else {
            self.heavy_low_mem
        }
    }

    const fn dem_limit(self, pages: usize) -> usize {
        if pages >= self.throttle_pages() {
            self.dem_high_mem
        } else {
            self.dem_low_mem
        }
    }

    const fn shared_limit(self, pages: usize) -> usize {
        if pages >= self.throttle_pages() {
            self.shared_high_mem
        } else {
            self.shared_low_mem
        }
    }
}

#[cfg(any(test, all(target_arch = "wasm32", target_feature = "atomics")))]
const ACTIVE_MT_MEMORY_POLICY: MtMemoryPolicy = MtMemoryPolicy::for_device_memory_gib(None);

#[cfg(any(test, all(target_arch = "wasm32", target_feature = "atomics")))]
const fn heavy_tile_limit_for_pages(pages: usize) -> usize {
    ACTIVE_MT_MEMORY_POLICY.heavy_limit(pages)
}

#[cfg(any(test, all(target_arch = "wasm32", target_feature = "atomics")))]
const fn dem_tile_limit_for_pages(pages: usize) -> usize {
    ACTIVE_MT_MEMORY_POLICY.dem_limit(pages)
}

#[cfg(any(test, all(target_arch = "wasm32", target_feature = "atomics")))]
fn current_dem_tile_limit() -> usize {
    #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
    {
        dem_tile_limit_for_pages(core::arch::wasm32::memory_size::<0>())
    }
    #[cfg(not(all(target_arch = "wasm32", target_feature = "atomics")))]
    {
        ACTIVE_MT_MEMORY_POLICY.dem_low_mem
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

#[cfg(any(test, all(target_arch = "wasm32", target_feature = "atomics")))]
const fn shared_tile_limit_for_pages(pages: usize) -> usize {
    ACTIVE_MT_MEMORY_POLICY.shared_limit(pages)
}

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
fn current_shared_tile_limit() -> usize {
    shared_tile_limit_for_pages(core::arch::wasm32::memory_size::<0>())
}

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

#[cfg(any(test, all(target_arch = "wasm32", target_feature = "atomics")))]
static DEM_LIMITER: LazyLock<Mutex<PermitState>> =
    LazyLock::new(|| Mutex::new(PermitState::default()));

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
static SHARED_TILES: LazyLock<Mutex<PermitState>> =
    LazyLock::new(|| Mutex::new(PermitState::default()));

async fn acquire_slot(gate: &'static Mutex<PermitState>, mut limit: impl FnMut() -> usize) {
    poll_fn(|cx| crate::util::sync::lock_unpoisoned(gate).poll_acquire(limit(), cx.waker())).await;
}

fn release_slot(gate: &'static Mutex<PermitState>) {
    let waiters = crate::util::sync::lock_unpoisoned(gate).release();
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

/// RAII token bounding concurrent DEM tile decodes on `wasm32 + atomics`.
#[cfg(any(test, all(target_arch = "wasm32", target_feature = "atomics")))]
pub(crate) struct DemTileSlot {
    _private: (),
}

#[cfg(any(test, all(target_arch = "wasm32", target_feature = "atomics")))]
impl Drop for DemTileSlot {
    fn drop(&mut self) {
        release_slot(&DEM_LIMITER);
    }
}

/// Acquires one dedicated DEM decode slot so terrain elevation tiles are never starved behind
/// heavy vector MVT decodes.
#[cfg(any(test, all(target_arch = "wasm32", target_feature = "atomics")))]
pub(crate) async fn acquire_dem_tile_slot() -> DemTileSlot {
    acquire_slot(&DEM_LIMITER, current_dem_tile_limit).await;
    DemTileSlot { _private: () }
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
    acquire_slot(&SHARED_TILES, current_shared_tile_limit).await;
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
    #[cfg(feature = "raster")]
    pub(crate) fn merge(&mut self, other: Self) {
        #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
        if self.permit.is_none() {
            self.permit = other.permit;
        }
        #[cfg(not(all(target_arch = "wasm32", target_feature = "atomics")))]
        let _ = (self, other);
    }

    /// Extracts any held permit so the caller can attach it to an APC outcome message.
    #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
    pub(crate) fn into_permit(self) -> Option<SharedTilePermit> {
        self.permit
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

#[cfg(test)]
mod tests;
