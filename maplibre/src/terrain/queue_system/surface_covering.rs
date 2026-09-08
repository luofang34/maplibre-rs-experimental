//! Texture budgets do not change the mesh or DEM resolution under a surface.
use crate::{coords::WorldTileCoords, tcs::world::World, terrain::drape_targets::TargetSpec};

#[derive(Default)]
pub(super) struct SurfaceTiles(pub Vec<WorldTileCoords>);

pub(super) fn for_frame(
    world: &World,
    drapes: &[TargetSpec],
    sources: &[Option<WorldTileCoords>],
) -> (Vec<TargetSpec>, Vec<Option<WorldTileCoords>>) {
    let Some(surface) = world.resources.get::<SurfaceTiles>() else {
        return (Vec::new(), Vec::new());
    };
    let specs = surface
        .0
        .iter()
        .map(|coords| TargetSpec {
            coords: *coords,
            shapes: Vec::new(),
        })
        .collect();
    let mapped = map_sources(&surface.0, drapes, sources);
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
