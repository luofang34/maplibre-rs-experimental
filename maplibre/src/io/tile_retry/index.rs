//! Sparse active-request index for `O(|in_flight| + |retrying|)` per-frame scheduling.

use std::{collections::HashSet, time::Duration};

use rustc_hash::FxHashSet;

use super::{observed_now, RequestKind, RetryState, TileRequestRetries};
use crate::{coords::WorldTileCoords, tcs::world::World};

/// Tracks only tiles with an in-flight attempt or an active retry deadline per [`RequestKind`],
/// avoiding full-store scans across settled cached tiles on every frame.
#[derive(Default)]
pub(super) struct RetryIndex {
    /// Tiles whose `RetryState::pending` is `true` for each [`RequestKind::index`].
    pub(super) pending: [FxHashSet<WorldTileCoords>; 3],
    /// Tiles whose `RetryState::deadline` is `Some(_)` for each [`RequestKind::index`].
    pub(super) scheduled: [FxHashSet<WorldTileCoords>; 3],
    /// Last wanted set recorded by [`want`] for each [`RequestKind::index`].
    pub(super) last_wanted: [Option<FxHashSet<WorldTileCoords>>; 3],
}

/// The tiles each request family considered this frame; a family that did not run leaves
/// its requests alone.
#[derive(Default)]
struct WantedRequests([Option<FxHashSet<WorldTileCoords>>; 3]);

/// Attempts given up on, for the next request system to stop in its worker.
#[derive(Default)]
struct CancelledAttempts(Vec<u64>);

impl RetryIndex {
    /// Whether `coords` is currently outside the most recent `want()` set for `kind`.
    pub(super) fn is_out_of_view(&self, kind: RequestKind, coords: WorldTileCoords) -> bool {
        self.last_wanted[kind.index()]
            .as_ref()
            .is_some_and(|wanted| !wanted.contains(&coords))
    }

    pub(super) fn mark_pending(&mut self, kind: RequestKind, coords: WorldTileCoords) {
        let idx = kind.index();
        self.scheduled[idx].remove(&coords);
        self.pending[idx].insert(coords);
    }

    pub(super) fn finish_pending(
        &mut self,
        kind: RequestKind,
        coords: WorldTileCoords,
        has_deadline: bool,
    ) {
        let idx = kind.index();
        self.pending[idx].remove(&coords);
        if has_deadline {
            self.scheduled[idx].insert(coords);
        } else {
            self.scheduled[idx].remove(&coords);
        }
    }

    pub(super) fn mark_scheduled(&mut self, kind: RequestKind, coords: WorldTileCoords) {
        self.scheduled[kind.index()].insert(coords);
    }

    pub(super) fn clear_kind(&mut self, kind: RequestKind, coords: WorldTileCoords) {
        let idx = kind.index();
        self.pending[idx].remove(&coords);
        self.scheduled[idx].remove(&coords);
    }
}

pub(super) fn needs_frame_indexed(world: &World) -> bool {
    let Some(index) = world.resources.get::<RetryIndex>() else {
        return false;
    };
    for (kind_idx, pending) in index.pending.iter().enumerate() {
        for &coords in pending {
            if world
                .tiles
                .query::<&TileRequestRetries>(coords)
                .is_some_and(|retries| retries.0[kind_idx].pending)
            {
                return true;
            }
        }
    }
    if index.scheduled.iter().all(FxHashSet::is_empty) {
        return false;
    }
    let now = observed_now(world);
    for (kind_idx, scheduled) in index.scheduled.iter().enumerate() {
        for &coords in scheduled {
            if world
                .tiles
                .query::<&TileRequestRetries>(coords)
                .is_some_and(|retries| retries.0[kind_idx].retry_due(now))
            {
                return true;
            }
        }
    }
    false
}

pub(super) fn next_retry_delay_indexed(world: &World) -> Option<Duration> {
    let index = world.resources.get::<RetryIndex>()?;
    if index.scheduled.iter().all(FxHashSet::is_empty) {
        return None;
    }
    let now = observed_now(world);
    let mut shortest: Option<Duration> = None;
    for (kind_idx, scheduled) in index.scheduled.iter().enumerate() {
        for &coords in scheduled {
            let Some(retries) = world.tiles.query::<&TileRequestRetries>(coords) else {
                continue;
            };
            let state = &retries.0[kind_idx];
            if !state.out_of_view {
                if let Some(due) = state.deadline {
                    let wait = due.saturating_sub(now);
                    shortest = Some(shortest.map_or(wait, |prev| prev.min(wait)));
                }
            }
        }
    }
    shortest
}

/// Records the tiles a request family still wants, whether or not it requested them now.
pub(crate) fn want<S: std::hash::BuildHasher>(
    world: &mut World,
    kind: RequestKind,
    coords: &HashSet<WorldTileCoords, S>,
) {
    let idx = kind.index();
    let wanted_set: FxHashSet<WorldTileCoords> = coords.iter().copied().collect();
    let active: Vec<WorldTileCoords> = world
        .resources
        .get::<RetryIndex>()
        .map(|retry_idx| {
            retry_idx.pending[idx]
                .iter()
                .chain(retry_idx.scheduled[idx].iter())
                .copied()
                .collect()
        })
        .unwrap_or_default();
    let mut missing = Vec::new();
    for held in active {
        if let Some(retries) = world.tiles.query_mut::<&mut TileRequestRetries>(held) {
            retries.0[idx].out_of_view = !wanted_set.contains(&held);
        } else {
            missing.push(held);
        }
    }
    let retry_idx = world.resources.get_or_init_mut::<RetryIndex>();
    for gone in missing {
        retry_idx.clear_kind(kind, gone);
    }
    retry_idx.last_wanted[idx] = Some(wanted_set.clone());
    world.resources.get_or_init_mut::<WantedRequests>().0[idx] = Some(wanted_set);
}

/// Gives up the requests in flight that no request family wanted this frame, so they stop
/// holding a slot, and returns the tiles they were for. Tiles in `in_use` keep theirs: the
/// drape requests a family follows lag a frame behind what the view uses.
pub(crate) fn cancel_unwanted<S: std::hash::BuildHasher>(
    world: &mut World,
    in_use: &HashSet<WorldTileCoords, S>,
) -> Vec<WorldTileCoords> {
    let Some(wanted) = world
        .resources
        .get_mut::<WantedRequests>()
        .map(std::mem::take)
    else {
        return Vec::new();
    };
    let mut candidate_set = FxHashSet::default();
    if let Some(retry_idx) = world.resources.get::<RetryIndex>() {
        for pending in &retry_idx.pending {
            for &coords in pending {
                if !in_use.contains(&coords) {
                    candidate_set.insert(coords);
                }
            }
        }
    }
    if candidate_set.is_empty() {
        return Vec::new();
    }
    let mut coords_list: Vec<WorldTileCoords> = candidate_set.into_iter().collect();
    coords_list.sort_by_key(|c| c.build_quad_key());
    let mut cancelled = Vec::new();
    let mut tiles = Vec::new();
    let mut given_up = Vec::new();
    let mut missing = Vec::new();
    for coords in coords_list {
        let Some(retries) = world.tiles.query_mut::<&mut TileRequestRetries>(coords) else {
            missing.push(coords);
            continue;
        };
        for ((state, wanted), kind) in retries.0.iter_mut().zip(&wanted.0).zip([
            RequestKind::Raster,
            RequestKind::Dem,
            RequestKind::Vector,
        ]) {
            let unwanted = wanted.as_ref().is_some_and(|set| !set.contains(&coords));
            if state.pending && unwanted {
                cancelled.extend(state.attempt);
                *state = RetryState::default();
                given_up.push((coords, kind));
                if !tiles.contains(&coords) {
                    tiles.push(coords);
                }
            }
        }
    }
    if !given_up.is_empty() || !missing.is_empty() {
        let retry_idx = world.resources.get_or_init_mut::<RetryIndex>();
        for &(coords, kind) in &given_up {
            retry_idx.clear_kind(kind, coords);
        }
        for gone in missing {
            for kind in [RequestKind::Raster, RequestKind::Dem, RequestKind::Vector] {
                retry_idx.clear_kind(kind, gone);
            }
        }
    }
    for (coords, kind) in given_up {
        crate::io::tile_load::abandoned(world, coords, kind);
    }
    world
        .resources
        .get_or_init_mut::<CancelledAttempts>()
        .0
        .extend(cancelled);
    tiles
}

/// Stops the worker calls of the attempts cancelled since the last call.
pub(crate) fn stop_cancelled<K: crate::environment::OffscreenKernel>(
    world: &mut World,
    apc: &impl crate::io::apc::AsyncProcedureCall<K>,
) {
    let cancelled = world
        .resources
        .get_mut::<CancelledAttempts>()
        .map(|cancelled| std::mem::take(&mut cancelled.0))
        .unwrap_or_default();
    for attempt in cancelled {
        apc.cancel(attempt);
    }
}
