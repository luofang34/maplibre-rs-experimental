//! Hierarchical tile pyramid with `O(1)` zoom-level bitmasks and contiguous Morton subtrees.
//!
//! Modeled after `maplibre-native` `TilePyramid` and `maplibre-gl-js` `SourceCache`, adapted to
//! Rust with per-zoom-level MSB-first 64-bit Morton [`BTreeMap`]s and a 32-bit `zoom_mask`.
//! Because standard Morton interleaving places every descendant of `(x, y, z)` at level `z + d`
//! inside one exact contiguous key range `[base ..= base | ((1 << 2d) - 1)]`, ancestor walks skip
//! empty levels in `O(1)` via bit-scans and descendant subtree checks avoid probing `4^d` missing
//! child coordinates.

mod cover;

use std::collections::BTreeMap;

pub use cover::PyramidCover;

use crate::coords::{WorldTileCoords, MAX_ZOOM, ZOOM_BOUNDS};

/// Interleaves 32-bit `x` (even bits) and `y` (odd bits) MSB-first into a 64-bit Morton key.
#[inline]
pub const fn morton_xy(x: u32, y: u32) -> u64 {
    (spread_bits_32(y) << 1) | spread_bits_32(x)
}

/// De-interleaves a 64-bit Morton key produced by [`morton_xy`] back into `(x, y)`.
#[inline]
pub const fn unmorton_xy(code: u64) -> (u32, u32) {
    (compact_bits_32(code), compact_bits_32(code >> 1))
}

#[inline]
const fn spread_bits_32(v: u32) -> u64 {
    let mut x = v as u64;
    x = (x | (x << 16)) & 0x0000_FFFF_0000_FFFF;
    x = (x | (x << 8)) & 0x00FF_00FF_00FF_00FF;
    x = (x | (x << 4)) & 0x0F0F_0F0F_0F0F_0F0F;
    x = (x | (x << 2)) & 0x3333_3333_3333_3333;
    x = (x | (x << 1)) & 0x5555_5555_5555_5555;
    x
}

#[inline]
const fn compact_bits_32(mut x: u64) -> u32 {
    x &= 0x5555_5555_5555_5555;
    x = (x | (x >> 1)) & 0x3333_3333_3333_3333;
    x = (x | (x >> 2)) & 0x0F0F_0F0F_0F0F_0F0F;
    x = (x | (x >> 4)) & 0x00FF_00FF_00FF_00FF;
    x = (x | (x >> 8)) & 0x0000_FFFF_0000_FFFF;
    x = (x | (x >> 16)) & 0x0000_0000_FFFF_FFFF;
    x as u32
}

#[inline]
pub(crate) fn canonical_key(coords: WorldTileCoords) -> Option<(usize, u64)> {
    let z = usize::from(u8::from(coords.z));
    let bounds = *ZOOM_BOUNDS.get(z)?;
    if coords.x < 0 || coords.y < 0 {
        return None;
    }
    let x = coords.x as u32;
    let y = coords.y as u32;
    if x >= bounds || y >= bounds {
        return None;
    }
    Some((z, morton_xy(x, y)))
}

#[derive(Clone, Debug)]
pub(crate) struct PyramidEntry<T> {
    pub(crate) coords: WorldTileCoords,
    pub(crate) value: T,
    pub(crate) last_touched: u64,
}

/// Multi-zoom tile pyramid indexed by per-level MSB-first Morton codes.
#[derive(Clone, Debug)]
pub struct TilePyramid<T> {
    pub(crate) levels: [BTreeMap<u64, PyramidEntry<T>>; MAX_ZOOM],
    pub(crate) zoom_mask: u32,
    pub(crate) len: usize,
    clock: u64,
}

impl<T> Default for TilePyramid<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> TilePyramid<T> {
    /// Creates an empty pyramid with zero heap allocations.
    pub const fn new() -> Self {
        Self {
            levels: [const { BTreeMap::new() }; MAX_ZOOM],
            zoom_mask: 0,
            len: 0,
            clock: 0,
        }
    }

    /// Number of canonical tiles currently stored across all zoom levels.
    #[inline]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Whether the pyramid holds no tiles.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Bitmask of populated zoom levels (`bit z == 1` iff level `z` is non-empty).
    #[inline]
    pub const fn zoom_mask(&self) -> u32 {
        self.zoom_mask
    }

    /// Coarsest populated zoom level in `O(1)`, or `None` when empty.
    #[inline]
    pub const fn min_zoom(&self) -> Option<u8> {
        if self.zoom_mask == 0 {
            None
        } else {
            Some(self.zoom_mask.trailing_zeros() as u8)
        }
    }

    /// Finest populated zoom level in `O(1)`, or `None` when empty.
    #[inline]
    pub const fn max_zoom(&self) -> Option<u8> {
        if self.zoom_mask == 0 {
            None
        } else {
            Some((31 - self.zoom_mask.leading_zeros()) as u8)
        }
    }

    /// Whether a canonical tile at `coords` is present.
    #[inline]
    pub fn contains(&self, coords: WorldTileCoords) -> bool {
        let Some((z, code)) = canonical_key(coords) else {
            return false;
        };
        (self.zoom_mask & (1_u32 << z)) != 0 && self.levels[z].contains_key(&code)
    }

    /// Borrows the value stored at canonical `coords`.
    #[inline]
    pub fn get(&self, coords: WorldTileCoords) -> Option<&T> {
        let (z, code) = canonical_key(coords)?;
        if (self.zoom_mask & (1_u32 << z)) == 0 {
            return None;
        }
        self.levels[z].get(&code).map(|entry| &entry.value)
    }

    /// Mutably borrows the value stored at canonical `coords`.
    #[inline]
    pub fn get_mut(&mut self, coords: WorldTileCoords) -> Option<&mut T> {
        let (z, code) = canonical_key(coords)?;
        if (self.zoom_mask & (1_u32 << z)) == 0 {
            return None;
        }
        self.levels[z].get_mut(&code).map(|entry| &mut entry.value)
    }

    /// Inserts or replaces `value` at canonical `coords`, returning any previous value.
    /// Out-of-grid coordinates are ignored and return `None`.
    pub fn insert(&mut self, coords: WorldTileCoords, value: T) -> Option<T> {
        let (z, code) = canonical_key(coords)?;
        self.clock = self.clock.wrapping_add(1);
        let touched = self.clock;
        let previous = self.levels[z].insert(
            code,
            PyramidEntry {
                coords,
                value,
                last_touched: touched,
            },
        );
        self.zoom_mask |= 1_u32 << z;
        if previous.is_none() {
            self.len += 1;
        }
        previous.map(|entry| entry.value)
    }

    /// Removes the tile at canonical `coords`, clearing its zoom bit when the level empties.
    pub fn remove(&mut self, coords: WorldTileCoords) -> Option<T> {
        let (z, code) = canonical_key(coords)?;
        if (self.zoom_mask & (1_u32 << z)) == 0 {
            return None;
        }
        let removed = self.levels[z].remove(&code)?;
        self.len -= 1;
        if self.levels[z].is_empty() {
            self.zoom_mask &= !(1_u32 << z);
        }
        Some(removed.value)
    }

    /// Clears every populated zoom level.
    pub fn clear(&mut self) {
        let mut mask = self.zoom_mask;
        while mask != 0 {
            let z = mask.trailing_zeros() as usize;
            self.levels[z].clear();
            mask &= mask - 1;
        }
        self.zoom_mask = 0;
        self.len = 0;
    }

    /// Marks `coords` as recently used; returns `true` when the tile was present.
    pub fn touch(&mut self, coords: WorldTileCoords) -> bool {
        let Some((z, code)) = canonical_key(coords) else {
            return false;
        };
        if (self.zoom_mask & (1_u32 << z)) == 0 {
            return false;
        }
        let next_clock = self.clock.wrapping_add(1);
        if let Some(entry) = self.levels[z].get_mut(&code) {
            self.clock = next_clock;
            entry.last_touched = next_clock;
            true
        } else {
            false
        }
    }

    /// Evicts least-recently-touched unprotected entries until at most `max_entries` unprotected
    /// entries remain, returning the evicted `(coords, value)` pairs.
    pub fn evict_lru(
        &mut self,
        max_entries: usize,
        mut is_protected: impl FnMut(WorldTileCoords, &T) -> bool,
    ) -> Vec<(WorldTileCoords, T)> {
        if self.len <= max_entries {
            return Vec::new();
        }
        let mut candidates: Vec<(u64, WorldTileCoords)> = Vec::new();
        let mut mask = self.zoom_mask;
        while mask != 0 {
            let z = mask.trailing_zeros() as usize;
            for entry in self.levels[z].values() {
                if !is_protected(entry.coords, &entry.value) {
                    candidates.push((entry.last_touched, entry.coords));
                }
            }
            mask &= mask - 1;
        }
        if candidates.len() <= max_entries {
            return Vec::new();
        }
        candidates.sort_unstable_by_key(|(tick, _)| *tick);
        let to_remove = candidates.len() - max_entries;
        candidates
            .into_iter()
            .take(to_remove)
            .filter_map(|(_, coords)| self.remove(coords).map(|val| (coords, val)))
            .collect()
    }

    /// Iterates `(coords, &value)` from coarsest zoom to finest zoom in Morton spatial order,
    /// visiting only populated zoom levels without intermediate allocation or sorting.
    pub fn iter_coarse_to_fine(&self) -> impl Iterator<Item = (WorldTileCoords, &T)> + '_ {
        PopulatedLevels::new(self.zoom_mask)
            .flat_map(move |z| self.levels[z].values().map(|e| (e.coords, &e.value)))
    }

    /// Iterates `coords` from coarsest zoom to finest zoom in Morton spatial order.
    pub fn coords_coarse_to_fine(&self) -> impl Iterator<Item = WorldTileCoords> + '_ {
        PopulatedLevels::new(self.zoom_mask)
            .flat_map(move |z| self.levels[z].values().map(|e| e.coords))
    }

    /// Iterates shared references to every stored value in coarse-to-fine Morton order.
    pub fn values(&self) -> impl Iterator<Item = &T> + '_ {
        PopulatedLevels::new(self.zoom_mask)
            .flat_map(move |z| self.levels[z].values().map(|e| &e.value))
    }

    /// Iterates mutable references to every stored value in coarse-to-fine order.
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut T> + '_ {
        self.levels
            .iter_mut()
            .flat_map(|level| level.values_mut().map(|e| &mut e.value))
    }
}

pub(crate) struct PopulatedLevels {
    mask: u32,
}

impl PopulatedLevels {
    #[inline]
    pub(crate) const fn new(mask: u32) -> Self {
        Self { mask }
    }
}

impl Iterator for PopulatedLevels {
    type Item = usize;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.mask == 0 {
            return None;
        }
        let z = self.mask.trailing_zeros() as usize;
        self.mask &= self.mask - 1;
        Some(z)
    }
}

#[cfg(test)]
mod tests;
