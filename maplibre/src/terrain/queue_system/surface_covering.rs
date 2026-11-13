//! Texture budgets do not change the mesh or DEM resolution under a surface.
use crate::{coords::WorldTileCoords, tcs::world::World, terrain::drape_targets::TargetSpec};

#[derive(Clone, Default)]
pub(crate) struct SurfaceTiles(pub Vec<WorldTileCoords>);

/// The terrain surface tiles the last frame's own selection drew.
pub(crate) fn drawn_surface_tiles(world: &World) -> &[WorldTileCoords] {
    world
        .resources
        .get::<SurfaceTiles>()
        .map_or(&[], |surface| surface.0.as_slice())
}

/// How many worlds over each surface tile is seen again: its copies beside the main world.
#[derive(Clone, Default)]
pub(crate) struct SurfaceCopies(pub std::collections::HashMap<WorldTileCoords, Vec<i32>>);

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
            expand_zoom_out_surfaces(world, surfaces, mapped)
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

fn expand_zoom_out_surfaces(
    world: &World,
    surfaces: Vec<WorldTileCoords>,
    mapped: Vec<Option<WorldTileCoords>>,
) -> (Vec<WorldTileCoords>, Vec<Option<WorldTileCoords>>) {
    use crate::{
        render::eventually::{Eventually, Eventually::Initialized},
        terrain::resources::TerrainResources,
    };
    let Some(Initialized(terrain)) = world.resources.get::<Eventually<TerrainResources>>() else {
        return (surfaces, mapped);
    };
    if !mapped.contains(&None) {
        return (surfaces, mapped);
    }
    let mut out_surfaces = Vec::with_capacity(surfaces.len());
    let mut out_sources = Vec::with_capacity(mapped.len());
    for (coords, source) in surfaces.into_iter().zip(mapped) {
        if source.is_some() || !has_descendant_drape(coords, terrain, 3) {
            out_surfaces.push(coords);
            out_sources.push(source);
        } else {
            subdivide_to_descendant_drapes(coords, terrain, 3, &mut out_surfaces, &mut out_sources);
        }
    }
    (out_surfaces, out_sources)
}

fn has_descendant_drape(
    coords: WorldTileCoords,
    terrain: &crate::terrain::resources::TerrainResources,
    depth: u8,
) -> bool {
    if depth == 0 || usize::from(u8::from(coords.z)) + 1 >= crate::coords::MAX_ZOOM {
        return false;
    }
    coords.get_children().into_iter().any(|child| {
        terrain.drape_texture(child).is_some() || has_descendant_drape(child, terrain, depth - 1)
    })
}

fn subdivide_to_descendant_drapes(
    coords: WorldTileCoords,
    terrain: &crate::terrain::resources::TerrainResources,
    depth: u8,
    out_surfaces: &mut Vec<WorldTileCoords>,
    out_sources: &mut Vec<Option<WorldTileCoords>>,
) {
    if terrain.drape_texture(coords).is_some() {
        out_surfaces.push(coords);
        out_sources.push(Some(coords));
        return;
    }
    if depth == 0
        || usize::from(u8::from(coords.z)) + 1 >= crate::coords::MAX_ZOOM
        || !has_descendant_drape(coords, terrain, depth)
    {
        out_surfaces.push(coords);
        out_sources.push(None);
        return;
    }
    for child in coords.get_children() {
        subdivide_to_descendant_drapes(child, terrain, depth - 1, out_surfaces, out_sources);
    }
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
