//! Ancestor bit-jumping and contiguous Morton subtree cover queries on [`TilePyramid`].

use crate::{
    coords::{WorldTileCoords, ZoomLevel, MAX_ZOOM},
    tcs::pyramid::{canonical_key, morton_xy, TilePyramid},
};

/// Covering tiles resolved from a [`TilePyramid`] for a target coordinate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PyramidCover {
    /// Target tile itself is resident.
    Exact(WorldTileCoords),
    /// Finer resident descendants completely cover every quadrant of the target.
    CompleteChildren(Vec<WorldTileCoords>),
    /// Nearest coarser resident ancestor covers the target.
    Ancestor(WorldTileCoords),
    /// Shallowest resident descendants cover part of the target when no ancestor is resident.
    PartialChildren(Vec<WorldTileCoords>),
    /// Neither the target, nor any ancestor, nor any descendant within range is resident.
    Empty,
}

impl PyramidCover {
    /// Converts the selection into its covering coordinates (`Empty` becomes an empty vector).
    pub fn into_coords(self) -> Vec<WorldTileCoords> {
        match self {
            Self::Exact(tile) | Self::Ancestor(tile) => vec![tile],
            Self::CompleteChildren(tiles) | Self::PartialChildren(tiles) => tiles,
            Self::Empty => Vec::new(),
        }
    }
}

/// Inclusive Morton range `[first ..= last]` of `coords`'s descendants `depth` levels finer.
#[inline]
fn subtree_morton_range(code: u64, depth: u32) -> (u64, u64) {
    let shift = 2 * depth;
    let first = code << shift;
    let last = first | ((1_u64 << shift) - 1);
    (first, last)
}

impl<T> TilePyramid<T> {
    /// Nearest strictly coarser resident ancestor (`z < coords.z`), skipping empty zoom levels
    /// via bit-scans on `zoom_mask`.
    #[inline]
    pub fn nearest_ancestor(&self, coords: WorldTileCoords) -> Option<(WorldTileCoords, &T)> {
        let (z, _) = canonical_key(coords)?;
        if z == 0 {
            return None;
        }
        let mut mask = self.zoom_mask & ((1_u32 << z) - 1);
        while mask != 0 {
            let az = (31 - mask.leading_zeros()) as usize;
            let shift = (z - az) as u32;
            let ax = (coords.x >> shift) as u32;
            let ay = (coords.y >> shift) as u32;
            if let Some(entry) = self.levels[az].get(&morton_xy(ax, ay)) {
                return Some((entry.coords, &entry.value));
            }
            mask &= !(1_u32 << az);
        }
        None
    }

    /// Returns `(coords, &value)` if `coords` itself is resident, otherwise [`Self::nearest_ancestor`].
    #[inline]
    pub fn nearest_ancestor_or_self(
        &self,
        coords: WorldTileCoords,
    ) -> Option<(WorldTileCoords, &T)> {
        if let Some(val) = self.get(coords) {
            return Some((coords, val));
        }
        self.nearest_ancestor(coords)
    }

    /// Returns `true` in `O(1)` (when finer levels are empty) or `O(log N)` (one contiguous
    /// Morton range probe per populated level) if any descendant of `coords` exists within
    /// `1..=max_depth` levels below `coords`.
    #[inline]
    pub fn has_descendants(&self, coords: WorldTileCoords, max_depth: usize) -> bool {
        if max_depth == 0 {
            return false;
        }
        let Some((z, code)) = canonical_key(coords) else {
            return false;
        };
        if z + 1 >= MAX_ZOOM {
            return false;
        }
        let end_z = z.saturating_add(max_depth).min(MAX_ZOOM - 1);
        let upper_mask = if end_z == 31 {
            u32::MAX
        } else {
            (1_u32 << (end_z + 1)) - 1
        };
        let lower_mask = (1_u32 << (z + 1)) - 1;
        let mut active = self.zoom_mask & upper_mask & !lower_mask;
        while active != 0 {
            let dz = active.trailing_zeros() as usize;
            let (first, last) = subtree_morton_range(code, (dz - z) as u32);
            if self.levels[dz].range(first..=last).next().is_some() {
                return true;
            }
            active &= active - 1;
        }
        false
    }

    /// Zero-allocation iterator over every resident descendant at exact relative `depth >= 1`
    /// (`coords.z + depth`), using a single contiguous Morton [`std::collections::BTreeMap::range`].
    pub fn descendants_at_depth(
        &self,
        coords: WorldTileCoords,
        depth: u8,
    ) -> impl Iterator<Item = (WorldTileCoords, &T)> + '_ {
        let slice = canonical_key(coords)
            .filter(|_| depth > 0)
            .and_then(|(z, code)| {
                let dz = z.checked_add(usize::from(depth))?;
                if dz >= MAX_ZOOM || (self.zoom_mask & (1_u32 << dz)) == 0 {
                    return None;
                }
                let (first, last) = subtree_morton_range(code, u32::from(depth));
                Some(self.levels[dz].range(first..=last))
            });
        slice
            .into_iter()
            .flatten()
            .map(|(_, entry)| (entry.coords, &entry.value))
    }

    /// Complete disjoint descendant cover up to `max_depth` levels down in clockwise
    /// [`WorldTileCoords::get_children`] order, or `None` if any quadrant is uncovered.
    pub fn complete_children(
        &self,
        coords: WorldTileCoords,
        max_depth: usize,
    ) -> Option<Vec<WorldTileCoords>> {
        if !self.has_descendants(coords, max_depth) {
            return None;
        }
        let mut out = Vec::with_capacity(4);
        self.collect_complete_children(coords, max_depth, &mut out)?;
        Some(out)
    }

    fn collect_complete_children(
        &self,
        coords: WorldTileCoords,
        depth: usize,
        out: &mut Vec<WorldTileCoords>,
    ) -> Option<()> {
        if depth == 0 || usize::from(u8::from(coords.z)) + 1 >= MAX_ZOOM {
            return None;
        }
        for child in coords.get_children() {
            if self.contains(child) {
                out.push(child);
            } else if depth > 1 && self.has_descendants(child, depth - 1) {
                self.collect_complete_children(child, depth - 1, out)?;
            } else {
                return None;
            }
        }
        Some(())
    }

    /// Shallowest resident descendants up to `max_depth` levels down in breadth-first clockwise
    /// [`WorldTileCoords::get_children`] order, pruning empty subtrees in `O(1)`.
    pub fn shallowest_descendants(
        &self,
        coords: WorldTileCoords,
        max_depth: usize,
    ) -> Vec<WorldTileCoords> {
        if !self.has_descendants(coords, max_depth) {
            return Vec::new();
        }
        let mut frontier = coords.get_children().to_vec();
        let mut output = Vec::new();
        for step in 0..max_depth {
            let remaining = max_depth - 1 - step;
            let mut next_frontier = Vec::new();
            for candidate in frontier {
                if self.contains(candidate) {
                    output.push(candidate);
                } else if remaining > 0 && self.has_descendants(candidate, remaining) {
                    next_frontier.extend(candidate.get_children());
                }
            }
            if next_frontier.is_empty() {
                break;
            }
            frontier = next_frontier;
        }
        output
    }

    /// Resolves the best covering tiles for `target` following `maplibre-native` `TilePyramid` /
    /// `maplibre-gl-js` `SourceCache` fallback order:
    /// 1. Exact tile (`target` itself),
    /// 2. Complete finer children up to `complete_depth`,
    /// 3. Nearest coarser ancestor,
    /// 4. Partial finer children up to `partial_depth`.
    pub fn resolve_cover(
        &self,
        target: WorldTileCoords,
        complete_depth: usize,
        partial_depth: usize,
    ) -> PyramidCover {
        if self.contains(target) {
            return PyramidCover::Exact(target);
        }
        if let Some(children) = self.complete_children(target, complete_depth) {
            return PyramidCover::CompleteChildren(children);
        }
        if let Some((ancestor, _)) = self.nearest_ancestor(target) {
            return PyramidCover::Ancestor(ancestor);
        }
        let partial = self.shallowest_descendants(target, partial_depth);
        if !partial.is_empty() {
            PyramidCover::PartialChildren(partial)
        } else {
            PyramidCover::Empty
        }
    }

    /// Convenience helper building ancestor coordinates at an explicit `level <= coords.z`.
    #[inline]
    pub fn ancestor_coords_at(coords: WorldTileCoords, level: u8) -> Option<WorldTileCoords> {
        let z = u8::from(coords.z);
        if level > z {
            return None;
        }
        let shift = u32::from(z - level);
        Some(WorldTileCoords {
            x: coords.x >> shift,
            y: coords.y >> shift,
            z: ZoomLevel::new(level),
        })
    }
}
