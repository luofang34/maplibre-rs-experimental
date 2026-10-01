//! Texture budgets do not change the mesh or DEM resolution under a surface.
use crate::{coords::WorldTileCoords, tcs::world::World, terrain::drape_targets::TargetSpec};

#[derive(Default)]
pub(super) struct SurfaceTiles(pub Vec<WorldTileCoords>);

/// How many worlds over each surface tile is seen again: its copies beside the main world.
#[derive(Default)]
pub(super) struct SurfaceCopies(pub std::collections::HashMap<WorldTileCoords, Vec<i32>>);

impl SurfaceCopies {
    /// The copies of the world the view sees its tiles in, keyed by the tile they repeat.
    pub(super) fn of(tiles: impl Iterator<Item = WorldTileCoords>) -> Self {
        let mut copies: std::collections::HashMap<WorldTileCoords, Vec<i32>> = Default::default();
        for (tile, wrap) in tiles.filter_map(WorldTileCoords::wrapped) {
            if wrap != 0 {
                let wraps = copies.entry(tile).or_default();
                if !wraps.contains(&wrap) {
                    wraps.push(wrap);
                }
            }
        }
        Self(copies)
    }
}

pub(super) fn for_frame(
    world: &mut World,
    drapes: &[TargetSpec],
    sources: &[Option<WorldTileCoords>],
) -> (Vec<TargetSpec>, Vec<Option<WorldTileCoords>>) {
    let surfaces = world
        .resources
        .get::<SurfaceTiles>()
        .map(|surface| surface.0.clone())
        .unwrap_or_default();
    let coordinates: Vec<_> = drapes.iter().map(|drape| drape.coords).collect();
    let (surfaces, mapped) =
        if let Some(active) = super::cohort::active_sources(world, &coordinates, sources) {
            super::cohort::surface_pieces(&surfaces, &active)
        } else {
            let mapped = map_sources(&surfaces, drapes, sources);
            (surfaces, mapped)
        };
    let specs = surfaces
        .into_iter()
        .map(|coords| TargetSpec {
            absent_sources: Vec::new(),
            coords,
            shapes: Vec::new(),
        })
        .collect();
    (specs, mapped)
}

fn map_sources(
    surfaces: &[WorldTileCoords],
    drapes: &[TargetSpec],
    sources: &[Option<WorldTileCoords>],
) -> Vec<Option<WorldTileCoords>> {
    surfaces
        .iter()
        .map(|coords| {
            drapes
                .iter()
                .zip(sources)
                .find(|(drape, _)| crate::projection::tile_covering::covers(drape.coords, *coords))
                .and_then(|(_, source)| *source)
        })
        .collect()
}

pub(super) fn fit_metadata(specs: &[TargetSpec], ready: &mut [bool], mut remaining: usize) {
    for (spec, ready) in specs.iter().zip(ready) {
        if !*ready {
            continue;
        }
        if spec.shapes.len() > remaining {
            *ready = false;
        } else {
            remaining -= spec.shapes.len();
        }
    }
}

#[cfg(test)]
mod tests;
