//! Per-tile CPU byte estimate cache so `Cleanup` avoids re-scanning unchanged cached tiles.

use rustc_hash::FxHashMap;

use crate::{coords::WorldTileCoords, render::tile_memory::tile_bytes, tcs::tiles::Tiles};

type ByteStamp = (Option<u64>, usize);

#[derive(Default)]
pub(super) struct TileBytesCache {
    entries: FxHashMap<WorldTileCoords, (ByteStamp, usize)>,
}

impl TileBytesCache {
    pub(super) fn get_or_compute(&mut self, tiles: &Tiles, coords: WorldTileCoords) -> usize {
        let stamp = (
            tiles.tile_content_rev(coords),
            tiles.geometry_index.tile_bytes(coords),
        );
        if let Some(&(cached_stamp, cached_bytes)) = self.entries.get(&coords) {
            if cached_stamp == stamp {
                return cached_bytes;
            }
        }
        let bytes = tile_bytes(tiles, coords);
        self.entries.insert(coords, (stamp, bytes));
        bytes
    }

    pub(super) fn remove(&mut self, coords: &WorldTileCoords) {
        self.entries.remove(coords);
    }

    pub(super) fn prune_if_oversized(&mut self, tiles: &Tiles) {
        if self.entries.len() > tiles.tiles.len() + 32 {
            self.entries.retain(|coords, _| tiles.exists(*coords));
        }
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }
}

pub(super) fn prune_last_used<S: std::hash::BuildHasher>(
    last_used: &mut FxHashMap<WorldTileCoords, u64>,
    tiles: &Tiles,
    in_use: &std::collections::HashSet<WorldTileCoords, S>,
) {
    if last_used.len() > tiles.tiles.len() * 2 + 64 {
        last_used.retain(|coords, _| tiles.exists(*coords) || in_use.contains(coords));
    }
}
