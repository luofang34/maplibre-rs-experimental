//! Coordinated texture detail for an external eye viewing a globe from above the atmosphere.
use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    tcs::world::World,
};
use std::collections::HashSet;

#[derive(Default)]
pub(super) struct TextureCohort {
    pub enabled: bool,
    desired: u8,
    active: u8,
}

pub(super) fn targets(
    world: &mut World,
    surfaces: &[WorldTileCoords],
    limit: usize,
) -> Vec<WorldTileCoords> {
    let mut desired = surfaces
        .iter()
        .map(|tile| u8::from(tile.z))
        .max()
        .unwrap_or(0);
    let targets = loop {
        if let Some(tiles) = at_level(surfaces, desired, limit) {
            let chain = ancestors(&tiles);
            if chain.len() <= limit.max(1) {
                break chain;
            }
        }
        if desired == 0 {
            break vec![WorldTileCoords::default()];
        }
        desired -= 1;
    };
    let state = world.resources.get_or_init_mut::<TextureCohort>();
    state.enabled = true;
    state.desired = desired;
    targets
}

pub(super) fn at_level(
    surfaces: &[WorldTileCoords],
    level: u8,
    limit: usize,
) -> Option<Vec<WorldTileCoords>> {
    let mut result = HashSet::new();
    for tile in surfaces {
        let delta = i32::from(level) - i32::from(u8::from(tile.z));
        if delta <= 0 {
            result.insert(WorldTileCoords {
                x: tile.x >> -delta,
                y: tile.y >> -delta,
                z: ZoomLevel::new(level),
            });
        } else {
            let count = 1_i32.checked_shl(delta as u32)?;
            if i64::from(count) * i64::from(count) > limit as i64 {
                return None;
            }
            for y in tile.y * count..(tile.y + 1) * count {
                for x in tile.x * count..(tile.x + 1) * count {
                    result.insert(WorldTileCoords {
                        x,
                        y,
                        z: ZoomLevel::new(level),
                    });
                }
            }
        }
        if result.len() > limit {
            return None;
        }
    }
    let mut tiles: Vec<_> = result.into_iter().collect();
    tiles.sort();
    Some(tiles)
}

fn ancestors(tiles: &[WorldTileCoords]) -> Vec<WorldTileCoords> {
    let mut all = HashSet::new();
    for tile in tiles {
        let mut current = Some(*tile);
        while let Some(tile) = current {
            all.insert(tile);
            current = tile.get_parent();
        }
    }
    let mut all: Vec<_> = all.into_iter().collect();
    all.sort_by_key(|tile| (tile.z, tile.x, tile.y));
    all
}

pub(super) fn active_sources(
    world: &mut World,
    tiles: &[WorldTileCoords],
    sources: &[Option<WorldTileCoords>],
) -> Option<Vec<(WorldTileCoords, Option<WorldTileCoords>)>> {
    let state = world.resources.get_or_init_mut::<TextureCohort>();
    if !state.enabled {
        return None;
    }
    let ready = (0..=state.desired)
        .rev()
        .find(|level| {
            let at_level: Vec<_> = tiles
                .iter()
                .zip(sources)
                .filter(|(tile, _)| u8::from(tile.z) == *level)
                .collect();
            !at_level.is_empty()
                && at_level
                    .iter()
                    .all(|(tile, source)| **source == Some(**tile))
        })
        .unwrap_or(0);
    // A newly exposed edge can use an ancestor while it loads; it must not make all
    // already visible terrain drop detail and then regain it on the next arrival.
    if ready >= state.active || state.desired < state.active {
        state.active = ready;
    }
    Some(
        tiles
            .iter()
            .copied()
            .zip(sources.iter().copied())
            .filter(|(tile, _)| u8::from(tile.z) == state.active)
            .collect(),
    )
}

pub(super) fn surface_pieces(
    surfaces: &[WorldTileCoords],
    sources: &[(WorldTileCoords, Option<WorldTileCoords>)],
) -> (Vec<WorldTileCoords>, Vec<Option<WorldTileCoords>>) {
    let mut pieces = Vec::new();
    let mut mapped = Vec::new();
    for surface in surfaces {
        for (tile, source) in sources {
            let piece = if crate::projection::tile_covering::covers(*tile, *surface) {
                *surface
            } else if crate::projection::tile_covering::covers(*surface, *tile) {
                *tile
            } else {
                continue;
            };
            pieces.push(piece);
            mapped.push(*source);
        }
    }
    (pieces, mapped)
}

#[cfg(test)]
mod tests;
