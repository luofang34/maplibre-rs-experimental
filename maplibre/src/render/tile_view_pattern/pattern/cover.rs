//! Source shape selection, covering overlap pairing, and sub-tile clip rectangles.

use rustc_hash::FxHashSet;

use super::{CHILDREN_SEARCH_DEPTH, COMPLETE_CHILDREN_SEARCH_DEPTH};
use crate::{
    coords::{WorldTileCoords, Zoom, ZoomLevel},
    render::tile_view_pattern::{HasTile, SourceShapes, TileShape},
    tcs::world::World,
};

/// The loaded shapes nearest to `coords` in the pyramid: the tile itself, complete children,
/// a parent, or whatever children exist. A parent already standing in for another view tile
/// is not drawn twice.
pub(super) fn source_shapes<T: HasTile>(
    container: &T,
    coords: WorldTileCoords,
    zoom: Zoom,
    world: &World,
    used_parents: &mut FxHashSet<WorldTileCoords>,
) -> SourceShapes {
    source_shapes_bounded(container, coords, zoom, world, used_parents, None)
}

pub(super) fn source_shapes_bounded<T: HasTile>(
    container: &T,
    coords: WorldTileCoords,
    zoom: Zoom,
    world: &World,
    used_parents: &mut FxHashSet<WorldTileCoords>,
    max_zoom: Option<u8>,
) -> SourceShapes {
    let max_child_depth = max_zoom.map_or(usize::MAX, |max_z| {
        usize::from(max_z.saturating_sub(u8::from(coords.z)))
    });
    let cover = if container.has_tile(coords, world) {
        Some(vec![coords])
    } else {
        container
            .get_complete_children(
                coords,
                world,
                COMPLETE_CHILDREN_SEARCH_DEPTH.min(max_child_depth),
            )
            .or_else(|| {
                container
                    .get_available_parent(coords, world)
                    .map(|parent| vec![parent])
            })
    };
    match cover.or_else(|| {
        container
            .get_available_children(coords, world, CHILDREN_SEARCH_DEPTH.min(max_child_depth))
            .filter(|tiles| !tiles.is_empty())
    }) {
        Some(tiles) if !tiles.is_empty() => selected_shapes(coords, tiles, zoom, used_parents),
        _ => SourceShapes::None,
    }
}

pub(super) fn selected_shapes(
    coords: WorldTileCoords,
    tiles: Vec<WorldTileCoords>,
    zoom: Zoom,
    used_parents: &mut FxHashSet<WorldTileCoords>,
) -> SourceShapes {
    if let [parent] = tiles.as_slice() {
        // Adjacent view tiles sharing an ancestor must not blend that ancestor repeatedly.
        if parent.z < coords.z && !used_parents.insert(*parent) {
            return SourceShapes::None;
        }
    }
    shapes_from_tiles(coords, tiles, zoom)
}

/// Shapes for covering tiles that overlap `coords`: the tile itself, its descendants, or the
/// one ancestor standing in for it.
fn shapes_from_tiles(
    coords: WorldTileCoords,
    tiles: Vec<WorldTileCoords>,
    zoom: Zoom,
) -> SourceShapes {
    match tiles.as_slice() {
        [tile] if *tile == coords => SourceShapes::SourceEqTarget(TileShape::new(coords, zoom)),
        [tile] if tile.z < coords.z => SourceShapes::Parent(TileShape::new(*tile, zoom)),
        _ => SourceShapes::Children(
            tiles
                .into_iter()
                .map(|tile| TileShape::new(tile, zoom))
                .collect(),
        ),
    }
}

/// The tiles of a covering that overlap a view tile: the tile itself, its descendants, or the
/// ancestor standing in for it, as GL JS `getTerrainCoords` pairs them.
pub fn covering_shapes_for(
    target: WorldTileCoords,
    covering: &[WorldTileCoords],
) -> Vec<WorldTileCoords> {
    covering
        .iter()
        .copied()
        .filter(|source| overlaps(target, *source))
        .collect()
}

fn overlaps(target: WorldTileCoords, source: WorldTileCoords) -> bool {
    if source.z >= target.z {
        ancestor_at(source, target.z) == Some(target)
    } else {
        ancestor_at(target, source.z) == Some(source)
    }
}

pub(super) fn ancestor_at(tile: WorldTileCoords, level: ZoomLevel) -> Option<WorldTileCoords> {
    let mut current = tile;
    while current.z > level {
        current = current.get_parent()?;
    }
    Some(current)
}

/// The square of its tile a shape draws, in tile coordinates: that of the finer tile it is
/// drawn within, or the whole tile.
pub(super) fn clip_region(shape: &TileShape) -> crate::render::shaders::Vec4f32 {
    let Some(within) = shape.within else {
        return crate::render::shaders::WHOLE_TILE;
    };
    let levels = i32::from(u8::from(within.z)) - i32::from(u8::from(shape.coords.z));
    if levels <= 0 || within.x >> levels != shape.coords.x || within.y >> levels != shape.coords.y {
        return crate::render::shaders::WHOLE_TILE;
    }
    let size = crate::coords::EXTENT / f64::from(1_u32 << levels);
    let x = f64::from(within.x - (shape.coords.x << levels)) * size;
    let y = f64::from(within.y - (shape.coords.y << levels)) * size;
    [x as f32, y as f32, (x + size) as f32, (y + size) as f32]
}
