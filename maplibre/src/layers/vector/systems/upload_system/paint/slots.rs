//! Tracks GPU layer-metadata slot contents across frames and tile cache re-entries.

use std::{
    hash::{Hash, Hasher},
    sync::Arc,
};

use rustc_hash::{FxHashMap, FxHasher};

use crate::{coords::WorldTileCoords, render::shaders::ShaderLayerMetadata};

#[inline]
pub(in super::super) fn source_tiles_hash(source_tiles: &[WorldTileCoords]) -> u64 {
    source_tiles
        .iter()
        .fold(source_tiles.len() as u64, |digest, coords| {
            let mut hasher = FxHasher::default();
            coords.hash(&mut hasher);
            digest.wrapping_add(hasher.finish())
        })
}

#[derive(Default)]
pub(in super::super) struct SlotMetaCache {
    last_style_rev: Option<u64>,
    last_eviction_gen: u64,
    last_source_hash: u64,
    has_source_hash: bool,
    last_source_arc: Option<Arc<[WorldTileCoords]>>,
    slots: FxHashMap<(u16, u64), ShaderLayerMetadata>,
}

impl SlotMetaCache {
    pub(in super::super) fn invalidate_all(&mut self) {
        self.slots.clear();
        self.has_source_hash = false;
        self.last_source_arc = None;
    }

    pub(in super::super) fn sync_generations(&mut self, style_rev: u64, eviction_gen: u64) {
        if self.last_style_rev != Some(style_rev) || self.last_eviction_gen != eviction_gen {
            self.last_style_rev = Some(style_rev);
            self.last_eviction_gen = eviction_gen;
            self.invalidate_all();
        }
    }

    /// Fast-path refresh check using `Arc::ptr_eq` before falling back to tile-set hashing.
    pub(in super::super) fn check_refresh_arc(
        &mut self,
        source_tiles: &Arc<[WorldTileCoords]>,
        refresh_paint: bool,
        settle_pending: bool,
    ) -> (bool, bool) {
        if self.has_source_hash
            && self
                .last_source_arc
                .as_ref()
                .is_some_and(|prev| Arc::ptr_eq(prev, source_tiles))
        {
            return (refresh_paint || settle_pending, false);
        }
        let (run, tiles_changed) = self.check_refresh(source_tiles, refresh_paint, settle_pending);
        if run && tiles_changed {
            self.last_source_arc = Some(Arc::clone(source_tiles));
        }
        (run, tiles_changed)
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
            self.last_source_arc = None;
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
