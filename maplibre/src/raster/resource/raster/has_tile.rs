//! [`HasTile`] implementation for [`RasterResources`].

use super::RasterResources;
use crate::{
    coords::WorldTileCoords, raster::RasterSourceId, render::tile_view_pattern::HasTile,
    tcs::world::World,
};

impl HasTile for RasterResources {
    fn has_source_tile(
        &self,
        source: &RasterSourceId,
        coords: WorldTileCoords,
        _world: &World,
    ) -> bool {
        self.get_bound_texture(source, &coords).is_some()
    }

    fn has_tile(&self, coords: WorldTileCoords, _world: &World) -> bool {
        self.bound_textures
            .values()
            .any(|textures| textures.contains(coords))
    }

    fn nearest_source_ancestor_candidate(
        &self,
        source: &RasterSourceId,
        coords: WorldTileCoords,
        _world: &World,
    ) -> Option<WorldTileCoords> {
        self.bound_textures
            .get(source)?
            .nearest_ancestor(coords)
            .map(|(ancestor, _)| ancestor)
    }

    fn has_source_descendant_candidates(
        &self,
        source: &RasterSourceId,
        coords: WorldTileCoords,
        max_depth: usize,
        _world: &World,
    ) -> bool {
        self.bound_textures
            .get(source)
            .is_some_and(|textures| textures.has_descendants(coords, max_depth))
    }

    fn nearest_ancestor_candidate(
        &self,
        coords: WorldTileCoords,
        _world: &World,
    ) -> Option<WorldTileCoords> {
        self.bound_textures
            .values()
            .filter_map(|textures| textures.nearest_ancestor(coords).map(|(c, _)| c))
            .max_by_key(|c| c.z)
    }

    fn has_descendant_candidates(
        &self,
        coords: WorldTileCoords,
        max_depth: usize,
        _world: &World,
    ) -> bool {
        self.bound_textures
            .values()
            .any(|textures| textures.has_descendants(coords, max_depth))
    }
}
