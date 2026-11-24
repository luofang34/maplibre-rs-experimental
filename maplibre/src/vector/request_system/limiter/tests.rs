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

    // Waiters block at limit, deduplicate identical wakers, and all wake on release so an
    // aborted front waiter cannot starve followers.
    let counter2 = Arc::new(CountWake(AtomicUsize::new(0)));
    let waker2 = Waker::from(counter2.clone());
    assert!(state.poll_acquire(2, &waker).is_pending());
    assert!(state.poll_acquire(2, &waker).is_pending());
    assert!(state.poll_acquire(2, &waker2).is_pending());
    assert_eq!(state.waiters.len(), 2);

    for w in state.release() {
        w.wake();
    }
    assert_eq!(state.in_use, 1);
    assert_eq!(counter.0.load(Ordering::Relaxed), 1);
    assert_eq!(counter2.0.load(Ordering::Relaxed), 1);
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
    assert_eq!(heavy_tile_limit_for_pages(383), 4);
    assert_eq!(heavy_tile_limit_for_pages(384), 2);
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
        .poll_acquire(heavy_tile_limit_for_pages(384), &waker)
        .is_pending());
    for w in state.release() {
        w.wake();
    }
    assert_eq!(state.in_use, 3);
    assert!(state
        .poll_acquire(heavy_tile_limit_for_pages(384), &waker)
        .is_pending());
    for w in state.release() {
        w.wake();
    }
    assert_eq!(state.in_use, 2);
    assert!(state
        .poll_acquire(heavy_tile_limit_for_pages(384), &waker)
        .is_pending());
    for w in state.release() {
        w.wake();
    }
    assert_eq!(state.in_use, 1);
    assert!(state
        .poll_acquire(heavy_tile_limit_for_pages(384), &waker)
        .is_ready());
    assert_eq!(state.in_use, 2);
}

#[test]
fn shared_and_dem_tile_limits_tighten_at_memory_threshold() {
    assert_eq!(shared_tile_limit_for_pages(0), 8);
    assert_eq!(shared_tile_limit_for_pages(383), 8);
    assert_eq!(shared_tile_limit_for_pages(384), 4);
    assert_eq!(shared_tile_limit_for_pages(1024), 4);

    assert_eq!(dem_tile_limit_for_pages(0), 2);
    assert_eq!(dem_tile_limit_for_pages(383), 2);
    assert_eq!(dem_tile_limit_for_pages(384), 1);
    assert_eq!(dem_tile_limit_for_pages(1024), 1);
}

#[test]
fn dem_limiter_is_independent_from_heavy_vector_limiter() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime");
    rt.block_on(async {
        let _heavy = acquire_heavy_tile_slot().await;
        let dem1 = acquire_dem_tile_slot().await;
        let dem2 = acquire_dem_tile_slot().await;
        drop((dem1, dem2));
        let _dem3 = acquire_dem_tile_slot().await;
    });
}
