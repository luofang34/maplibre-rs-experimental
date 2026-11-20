//! Host-thread remapping for worker-emitted `shared_geometry_key` values.
//!
//! Single-threaded Web Workers run in isolated WASM linear memories whose process-local
//! `AtomicU64` counters each start at `1`. Remapping `(coords, attempt, raw_key)` on arrival on
//! the render thread prevents distinct worker attempts from colliding in `VectorBufferPool` while
//! preserving sharing across sibling layers within the same tile attempt.

use rustc_hash::FxHashMap;

use crate::coords::WorldTileCoords;

/// Maps `(coords, attempt, raw_worker_key)` to a render-thread unique `u64` key.
pub(crate) struct SharedGeometryKeyRemap {
    next_key: u64,
    active: FxHashMap<WorldTileCoords, (Option<u64>, FxHashMap<u64, u64>)>,
}

impl Default for SharedGeometryKeyRemap {
    fn default() -> Self {
        Self {
            next_key: 1,
            active: FxHashMap::default(),
        }
    }
}

impl SharedGeometryKeyRemap {
    /// Returns the host-unique shared key for `(coords, attempt, raw_key)`.
    pub(crate) fn remap(
        &mut self,
        coords: WorldTileCoords,
        attempt: Option<u64>,
        raw_key: u64,
    ) -> u64 {
        let next = &mut self.next_key;
        let (stored_attempt, table) = self
            .active
            .entry(coords)
            .or_insert_with(|| (attempt, FxHashMap::default()));
        if *stored_attempt != attempt {
            *stored_attempt = attempt;
            table.clear();
        }
        *table.entry(raw_key).or_insert_with(|| {
            let key = *next;
            *next = next.wrapping_add(1).max(1);
            key
        })
    }

    /// Releases remap entries once the active attempt for `coords` completes.
    pub(crate) fn finish(&mut self, coords: WorldTileCoords, attempt: Option<u64>) {
        if self
            .active
            .get(&coords)
            .is_some_and(|(stored, _)| *stored == attempt)
        {
            self.active.remove(&coords);
        }
    }

    #[cfg(all(test, feature = "headless", feature = "thread-safe-futures"))]
    pub(crate) fn active_entry_count(&self) -> usize {
        self.active.values().map(|(_, table)| table.len()).sum()
    }
}
