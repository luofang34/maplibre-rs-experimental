//! Coverage-preserving refinement with a bounded tile working set.
use std::{
    cmp::Ordering,
    collections::{BinaryHeap, HashMap},
};

use cgmath::{InnerSpace, Point2, Vector3};

#[cfg(test)]
use crate::coords::LatLon;
use crate::coords::{TileCoords, WorldTileCoords, ZoomLevel};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum CoveringFocus {
    /// Unit-sphere focus vector `f_hat in S^2` for 3D Globe covering (`1.0 - c_hat.dot(f_hat)`).
    Spherical(Vector3<f64>),
    /// Planar Web Mercator focus in `0..1` world units (preserving `wrap = -1, 0, +1` copies).
    Planar(Point2<f64>),
}

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

/// Refine the most undersampled visible globe tile first on `S^2` (`1.0 - c_hat.dot(focus_unit)`).
/// A split replaces its parent only when all visible children fit, so the budget cannot cut holes.
pub(crate) fn bounded_spherical<E>(
    limit: usize,
    min_zoom: u8,
    focus_unit: Vector3<f64>,
    mut inspect: impl FnMut(WorldTileCoords, bool) -> Result<Option<Refinement>, E>,
) -> Result<Vec<WorldTileCoords>, E> {
    bounded_by(
        limit,
        min_zoom,
        0,
        CoveringFocus::Spherical(focus_unit),
        &mut inspect,
        |tile, refinement| {
            usize::from(u8::from(refinement.target).saturating_sub(u8::from(tile.z)))
        },
    )
}

/// Refine the most undersampled visible planar Mercator tile first, keeping distinct 2D world-copy
/// distances across `[-world_copies..=world_copies]`.
pub(crate) fn bounded_planar<E>(
    limit: usize,
    min_zoom: u8,
    world_copies: i32,
    focus_2d: Point2<f64>,
    mut inspect: impl FnMut(WorldTileCoords, bool) -> Result<Option<Refinement>, E>,
) -> Result<Vec<WorldTileCoords>, E> {
    bounded_by(
        limit,
        min_zoom,
        world_copies.max(0),
        CoveringFocus::Planar(focus_2d),
        &mut inspect,
        |tile, refinement| {
            usize::from(u8::from(refinement.target).saturating_sub(u8::from(tile.z)))
        },
    )
}

#[cfg(test)]
pub(crate) fn bounded<E>(
    limit: usize,
    min_zoom: u8,
    center: LatLon,
    inspect: impl FnMut(WorldTileCoords, bool) -> Result<Option<Refinement>, E>,
) -> Result<Vec<WorldTileCoords>, E> {
    bounded_spherical(
        limit,
        min_zoom,
        crate::projection::globe::lat_lon_to_unit_sphere(center),
        inspect,
    )
}

fn bounded_by<E>(
    limit: usize,
    min_zoom: u8,
    world_copies: i32,
    focus: CoveringFocus,
    mut inspect: impl FnMut(WorldTileCoords, bool) -> Result<Option<Refinement>, E>,
    rank: impl Fn(WorldTileCoords, &Refinement) -> usize,
) -> Result<Vec<WorldTileCoords>, E> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let mut roots = Vec::new();
    for wrap in -world_copies..=world_copies {
        let root = WorldTileCoords::from((wrap, 0, ZoomLevel::new(0)));
        if let Some(refinement) = inspect(root, false)? {
            roots.push(candidate(root, refinement, focus, &rank));
        }
    }
    if roots.is_empty() {
        return Ok(Vec::new());
    }
    roots.sort_by(|a, b| b.cmp(a));
    let mut pending: BinaryHeap<_> = roots.into_iter().take(limit).collect();
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
                children.push(candidate(tile, refinement, focus, &rank));
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

/// Singularity-free `S^2` angular priority key `1.0 - c_hat.dot(focus_unit)` in `[0, 2]`.
pub(crate) fn spherical_tile_distance(tile: WorldTileCoords, focus_unit: Vector3<f64>) -> f64 {
    let count = 1_i64 << u32::from(u8::from(tile.z));
    let canonical = TileCoords::from((
        i64::from(tile.x).rem_euclid(count) as u32,
        i64::from(tile.y).clamp(0, count - 1) as u32,
        tile.z,
    ));
    let cap = crate::projection::globe::covering::tile_spherical_cap(canonical);
    1.0 - cap.center.dot(focus_unit)
}

/// Planar 2D Web Mercator squared distance in world-copy coordinates (no wrap folding).
pub(crate) fn planar_tile_distance_squared(tile: WorldTileCoords, focus_2d: Point2<f64>) -> f64 {
    let count = tile.z.grid_scale();
    let dx = focus_2d.x - (f64::from(tile.x) + 0.5) / count;
    let dy = focus_2d.y - (f64::from(tile.y) + 0.5) / count;
    dx * dx + dy * dy
}

fn candidate(
    tile: WorldTileCoords,
    refinement: Refinement,
    focus: CoveringFocus,
    rank: &impl Fn(WorldTileCoords, &Refinement) -> usize,
) -> Candidate {
    let distance = match focus {
        CoveringFocus::Spherical(focus_unit) => spherical_tile_distance(tile, focus_unit),
        CoveringFocus::Planar(focus_2d) => planar_tile_distance_squared(tile, focus_2d),
    };
    Candidate {
        rank: rank(tile, &refinement),
        tile,
        refinement,
        distance,
    }
}

/// Fits a covering to the drape budget by refining the largest remaining detail deficit.
/// Distance breaks ties; a single nearby tile cannot consume every refinement slot.
#[cfg(test)]
pub(crate) fn coarsen(
    tiles: impl Iterator<Item = WorldTileCoords>,
    limit: usize,
    min_zoom: u8,
) -> Vec<WorldTileCoords> {
    coarsen_with_focus(tiles, limit, min_zoom, None)
}

pub(crate) fn coarsen_with_focus(
    tiles: impl Iterator<Item = WorldTileCoords>,
    limit: usize,
    min_zoom: u8,
    focus: Option<CoveringFocus>,
) -> Vec<WorldTileCoords> {
    let tiles: Vec<_> = tiles.collect();
    if tiles.len() <= limit {
        return tiles;
    }
    let mut ancestors = HashMap::new();
    let mut world_copies = 0_i32;
    for (index, tile) in tiles.iter().enumerate() {
        let mut ancestor = Some(*tile);
        while let Some(coords) = ancestor {
            if u8::from(coords.z) == 0 {
                world_copies = world_copies.max(coords.x.abs());
            }
            let entry = ancestors.entry(coords).or_insert((tile.z, index));
            entry.0 = entry.0.max(tile.z);
            ancestor = coords.get_parent();
        }
    }
    let focus = focus.unwrap_or_else(|| {
        let first = tiles[0];
        if world_copies == 0 {
            let count = 1_i64 << u32::from(u8::from(first.z));
            let canonical = TileCoords::from((
                i64::from(first.x).rem_euclid(count) as u32,
                i64::from(first.y).clamp(0, count - 1) as u32,
                first.z,
            ));
            CoveringFocus::Spherical(
                crate::projection::globe::covering::tile_spherical_cap(canonical).center,
            )
        } else {
            let count = first.z.grid_scale();
            CoveringFocus::Planar(Point2::new(
                (f64::from(first.x) + 0.5) / count,
                (f64::from(first.y) + 0.5) / count,
            ))
        }
    });
    let result: Result<_, std::convert::Infallible> = bounded_by(
        limit,
        min_zoom,
        world_copies,
        focus,
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
