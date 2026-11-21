//! Tracks GPU layer-metadata slot contents across frames and tile cache re-entries.

use std::hash::{Hash, Hasher};

use rustc_hash::{FxHashMap, FxHasher};

use crate::{coords::WorldTileCoords, render::shaders::ShaderLayerMetadata};

#[inline]
pub(in super::super) fn source_tiles_hash(source_tiles: &[WorldTileCoords]) -> u64 {
    let mut hasher = FxHasher::default();
    source_tiles.len().hash(&mut hasher);
    for coords in source_tiles {
        coords.hash(&mut hasher);
    }
    hasher.finish()
}

#[derive(Default)]
pub(in super::super) struct SlotMetaCache {
    last_style_rev: Option<u64>,
    last_eviction_gen: u64,
    last_source_hash: u64,
    has_source_hash: bool,
    slots: FxHashMap<(u16, u64), ShaderLayerMetadata>,
}

impl SlotMetaCache {
    pub(in super::super) fn invalidate_all(&mut self) {
        self.slots.clear();
        self.has_source_hash = false;
    }

    pub(in super::super) fn sync_generations(&mut self, style_rev: u64, eviction_gen: u64) {
        if self.last_style_rev != Some(style_rev) || self.last_eviction_gen != eviction_gen {
            self.last_style_rev = Some(style_rev);
            self.last_eviction_gen = eviction_gen;
            self.invalidate_all();
        }
    }

    /// Returns `(run_refresh, tiles_changed)` without mutating `last_source_hash` unless a refresh
    /// is actually going to run.
    pub(in super::super) fn check_refresh(
        &mut self,
        source_tiles: &[WorldTileCoords],
        refresh_paint: bool,
        settle_pending: bool,
    ) -> (bool, bool) {
        let hash = source_tiles_hash(source_tiles);
        let tiles_changed = !self.has_source_hash || self.last_source_hash != hash;
        let run = refresh_paint || settle_pending || tiles_changed;
        if run {
            self.last_source_hash = hash;
            self.has_source_hash = true;
        }
        (run, tiles_changed)
    }

    pub(in super::super) fn record(&mut self, slot: (u16, u64), meta: ShaderLayerMetadata) {
        self.slots.insert(slot, meta);
    }

    #[inline]
    pub(in super::super) fn is_stale(&self, slot: (u16, u64), meta: &ShaderLayerMetadata) -> bool {
        self.slots
            .get(&slot)
            .is_none_or(|cached| bytemuck::bytes_of(cached) != bytemuck::bytes_of(meta))
    }
}
