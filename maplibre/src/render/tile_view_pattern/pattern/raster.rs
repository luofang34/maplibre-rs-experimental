use rustc_hash::{FxHashMap, FxHashSet};

use super::{
    cover::{covering_shapes_for, selected_shapes, source_shapes},
    RasterCoverings,
};
use crate::{
    coords::{WorldTileCoords, Zoom},
    io::tile_sources::{RasterSourceId, TileKind},
    render::tile_view_pattern::{HasTile, SourceShapes, TileShape, ViewTileSources},
    tcs::world::World,
};

pub(super) fn generate_raster_shapes(
    raster_coverings: &RasterCoverings,
    sources: &ViewTileSources,
    coords: WorldTileCoords,
    (wrap, zoom): (i32, Zoom),
    world: &World,
    raster_parents: &mut FxHashMap<(RasterSourceId, i32), FxHashSet<WorldTileCoords>>,
) -> Vec<(RasterSourceId, SourceShapes)> {
    if raster_coverings.is_empty() {
        return Vec::new();
    }
    raster_coverings
        .iter()
        .map(|(source, covering)| {
            let availability = super::super::SourceTiles {
                source,
                availability: sources.of_kind(TileKind::Raster),
            };
            let covered = covering_shapes_for(coords, covering)
                .into_iter()
                .filter(|tile| availability.has_tile(*tile, world))
                .collect();
            let mut shapes = raster_shapes(
                &availability,
                coords,
                covered,
                zoom,
                world,
                raster_parents.entry((source.clone(), wrap)).or_default(),
            );
            if let Some(faded) = with_departing((&availability, source, &shapes), zoom, world) {
                shapes = faded;
            }
            shapes.for_each_mut(&mut |shape| shape.wrapped(wrap, zoom));
            (source.clone(), shapes)
        })
        .collect()
}

/// The shown tiles with the loaded descendants their source is fading out drawn over each of
/// them, as GL JS draws departing tiles over the tile that replaces them until the fade ends;
/// `None` when nothing under these shapes is departing.
#[cfg(feature = "raster")]
fn with_departing<T: HasTile>(
    (availability, source, shapes): (&T, &RasterSourceId, &SourceShapes),
    zoom: Zoom,
    world: &World,
) -> Option<SourceShapes> {
    let shown: Vec<WorldTileCoords> = match shapes {
        SourceShapes::SourceEqTarget(shape) => vec![shape.coords()],
        SourceShapes::Children(shapes) => shapes.iter().map(TileShape::coords).collect(),
        SourceShapes::Parent(_) | SourceShapes::None => return None,
    };
    let departing = world
        .resources
        .get::<crate::raster::cross_fade::RasterCrossFade>()?
        .departing(source.name())?;
    let mut tiles = Vec::new();
    let mut any = false;
    for tile in shown {
        tiles.push(TileShape::new(tile, zoom));
        let mut under: Vec<_> = departing
            .tiles
            .iter()
            .copied()
            .filter(|departed| {
                departed.z > tile.z
                    && super::cover::ancestor_at(*departed, tile.z) == Some(tile)
                    && availability.has_tile(*departed, world)
            })
            .collect();
        under.sort_by_key(|departed| (departed.z, departed.x, departed.y));
        any |= !under.is_empty();
        tiles.extend(
            under
                .into_iter()
                .map(|departed| TileShape::new(departed, zoom).within(tile)),
        );
    }
    any.then_some(SourceShapes::Children(tiles))
}

#[cfg(not(feature = "raster"))]
fn with_departing<T: HasTile>(
    _args: (&T, &RasterSourceId, &SourceShapes),
    _zoom: Zoom,
    _world: &World,
) -> Option<SourceShapes> {
    None
}

fn raster_shapes<T: HasTile>(
    sources: &T,
    coords: WorldTileCoords,
    covered: Vec<WorldTileCoords>,
    zoom: Zoom,
    world: &World,
    used_parents: &mut FxHashSet<WorldTileCoords>,
) -> SourceShapes {
    let covered = super::super::coverage::disjoint_tiles(coords, covered);
    let complete = super::super::coverage::covers_target(coords, &covered).then(|| covered.clone());
    if let Some(tiles) = complete {
        selected_shapes(coords, tiles, zoom, used_parents)
    } else if covered.is_empty() {
        match super::super::coverage::loaded_cover(sources, coords, world) {
            Some(tiles) => selected_shapes(coords, tiles, zoom, used_parents),
            None => source_shapes(sources, coords, zoom, world, used_parents),
        }
    } else {
        // What the view needs of this tile is loaded in part: the nearest loaded tile above or
        // at it stays underneath, and the part's own tiles are drawn over it.
        let mut tiles =
            super::super::coverage::loaded_cover(sources, coords, world).unwrap_or_default();
        tiles.retain(|tile| !covered.contains(tile));
        tiles.extend(covered);
        tiles.sort_by_key(|tile| tile.z);
        selected_shapes(coords, tiles, zoom, used_parents)
    }
}
