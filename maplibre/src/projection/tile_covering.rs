//! Coverage-preserving refinement with a bounded tile working set.
use crate::coords::{LatLon, WorldTileCoords, ZoomLevel};
use std::{
    cmp::Ordering,
    collections::{BinaryHeap, HashMap},
};

pub(crate) struct Refinement {
    pub(crate) target: ZoomLevel,
    pub(crate) fully_visible: bool,
}

struct Candidate {
    tile: WorldTileCoords,
    refinement: Refinement,
    distance: f64,
    rank: usize,
}

impl Candidate {
    fn priority(&self) -> u8 {
        u8::from(self.refinement.target).saturating_sub(u8::from(self.tile.z))
    }
}

impl PartialEq for Candidate {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Candidate {}
impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> Ordering {
        self.rank
            .cmp(&other.rank)
            .then_with(|| other.distance.total_cmp(&self.distance))
            .then_with(|| self.tile.cmp(&other.tile))
    }
}

/// Refine the most undersampled visible tile first. A split replaces its parent only when
/// all visible children fit, so the budget cannot cut holes in the visible surface.
pub(crate) fn bounded<E>(
    limit: usize,
    min_zoom: u8,
    center: LatLon,
    mut inspect: impl FnMut(WorldTileCoords, bool) -> Result<Option<Refinement>, E>,
) -> Result<Vec<WorldTileCoords>, E> {
    bounded_by(limit, min_zoom, center, &mut inspect, |tile, refinement| {
        usize::from(u8::from(refinement.target).saturating_sub(u8::from(tile.z)))
    })
}

fn bounded_by<E>(
    limit: usize,
    min_zoom: u8,
    center: LatLon,
    mut inspect: impl FnMut(WorldTileCoords, bool) -> Result<Option<Refinement>, E>,
    rank: impl Fn(WorldTileCoords, &Refinement) -> usize,
) -> Result<Vec<WorldTileCoords>, E> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let root = WorldTileCoords::from((0, 0, ZoomLevel::new(0)));
    let Some(refinement) = inspect(root, false)? else {
        return Ok(Vec::new());
    };
    let mut pending = BinaryHeap::from([candidate(root, refinement, center, &rank)]);
    let mut visible = Vec::new();
    // Conservative bounds may expose parents whose children all cull away. Count those
    // splits too, so freeing a frontier slot cannot turn an empty view into unbounded work.
    let mut remaining_splits = limit.saturating_mul(32);
    while let Some(parent) = pending.pop() {
        if parent.priority() == 0 || remaining_splits == 0 {
            visible.push(parent.tile);
            continue;
        }
        remaining_splits -= 1;
        let mut children = Vec::with_capacity(4);
        for tile in parent.tile.get_children() {
            if let Some(refinement) = inspect(tile, parent.refinement.fully_visible)? {
                children.push(candidate(tile, refinement, center, &rank));
            }
        }
        if visible.len() + pending.len() + children.len() <= limit {
            pending.extend(children);
        } else {
            visible.push(parent.tile);
        }
    }
    Ok(visible
        .into_iter()
        .filter(|tile| u8::from(tile.z) >= min_zoom)
        .collect())
}

fn candidate(
    tile: WorldTileCoords,
    refinement: Refinement,
    center: LatLon,
    rank: &impl Fn(WorldTileCoords, &Refinement) -> usize,
) -> Candidate {
    let count = 2_f64.powi(i32::from(u8::from(tile.z)));
    let center_x = center.longitude / 360.0 + 0.5;
    let latitude = center.latitude.clamp(-85.051_129, 85.051_129).to_radians();
    let center_y = (1.0 - latitude.tan().asinh() / std::f64::consts::PI) * 0.5;
    let dx = (center_x - (f64::from(tile.x) + 0.5) / count).abs();
    let dx = dx.min((1.0 - dx).abs());
    let dy = center_y - (f64::from(tile.y) + 0.5) / count;
    Candidate {
        rank: rank(tile, &refinement),
        tile,
        refinement,
        distance: dx * dx + dy * dy,
    }
}

/// Fits a covering to the drape budget by refining the largest remaining detail deficit.
/// Distance breaks ties; a single nearby tile cannot consume every refinement slot.
pub(crate) fn coarsen(
    tiles: impl Iterator<Item = WorldTileCoords>,
    limit: usize,
    min_zoom: u8,
) -> Vec<WorldTileCoords> {
    let tiles: Vec<_> = tiles.collect();
    if tiles.len() <= limit {
        return tiles;
    }
    let mut ancestors = HashMap::new();
    for (index, tile) in tiles.iter().enumerate() {
        let mut ancestor = Some(*tile);
        while let Some(coords) = ancestor {
            let entry = ancestors.entry(coords).or_insert((tile.z, index));
            entry.0 = entry.0.max(tile.z);
            ancestor = coords.get_parent();
        }
    }
    let first = tiles[0];
    let count = 2_f64.powi(i32::from(u8::from(first.z)));
    let center = LatLon::new(
        (std::f64::consts::PI * (1.0 - 2.0 * (f64::from(first.y) + 0.5) / count))
            .sinh()
            .atan()
            .to_degrees(),
        (f64::from(first.x) + 0.5) / count * 360.0 - 180.0,
    );
    let result: Result<_, std::convert::Infallible> = bounded_by(
        limit,
        min_zoom,
        center,
        |tile, _| {
            Ok(ancestors.get(&tile).map(|(target, _)| Refinement {
                target: *target,
                fully_visible: false,
            }))
        },
        |tile, refinement| {
            usize::from(u8::from(refinement.target).saturating_sub(u8::from(tile.z)))
        },
    );
    let mut result = match result {
        Ok(tiles) => tiles,
        Err(never) => match never {},
    };
    result.sort_by_key(|tile| (ancestors.get(tile).map(|(_, index)| *index), *tile));
    result
}

pub(crate) fn covers(parent: WorldTileCoords, tile: WorldTileCoords) -> bool {
    let delta = i32::from(u8::from(tile.z)) - i32::from(u8::from(parent.z));
    delta >= 0 && (tile.x >> delta) == parent.x && (tile.y >> delta) == parent.y
}

#[cfg(test)]
mod tests;
