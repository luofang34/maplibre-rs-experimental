//! Resource-query and multi-provider `HasTile` registries.

use std::marker::PhantomData;

use super::{coarser_ancestor, HasTile};
use crate::{
    coords::WorldTileCoords,
    io::tile_sources::{RasterSourceId, TileKind},
    tcs::{resources::ResourceQuery, world::World},
};

/// Resolves a resource query for each availability check; absent resources return `false`.
pub struct QueryHasTile<Q> {
    phantom_q: PhantomData<Q>,
}

impl<Q: ResourceQuery> Default for QueryHasTile<Q> {
    fn default() -> Self {
        Self {
            phantom_q: Default::default(),
        }
    }
}

impl<Q: ResourceQuery> HasTile for QueryHasTile<Q>
where
    for<'a> Q::Item<'a>: HasTile,
{
    fn has_tile(&self, coords: WorldTileCoords, world: &World) -> bool {
        world
            .resources
            .query::<Q>()
            .is_some_and(|resources| resources.has_tile(coords, world))
    }
    fn has_source_tile(
        &self,
        source: &RasterSourceId,
        coords: WorldTileCoords,
        world: &World,
    ) -> bool {
        world
            .resources
            .query::<Q>()
            .is_some_and(|resources| resources.has_source_tile(source, coords, world))
    }
    fn nearest_ancestor_candidate(
        &self,
        coords: WorldTileCoords,
        world: &World,
    ) -> Option<WorldTileCoords> {
        world
            .resources
            .query::<Q>()?
            .nearest_ancestor_candidate(coords, world)
    }
    fn nearest_source_ancestor_candidate(
        &self,
        source: &RasterSourceId,
        coords: WorldTileCoords,
        world: &World,
    ) -> Option<WorldTileCoords> {
        world
            .resources
            .query::<Q>()?
            .nearest_source_ancestor_candidate(source, coords, world)
    }
    fn has_descendant_candidates(
        &self,
        coords: WorldTileCoords,
        max_depth: usize,
        world: &World,
    ) -> bool {
        world
            .resources
            .query::<Q>()
            .is_some_and(|resources| resources.has_descendant_candidates(coords, max_depth, world))
    }
    fn has_source_descendant_candidates(
        &self,
        source: &RasterSourceId,
        coords: WorldTileCoords,
        max_depth: usize,
        world: &World,
    ) -> bool {
        world.resources.query::<Q>().is_some_and(|resources| {
            resources.has_source_descendant_candidates(source, coords, max_depth, world)
        })
    }
}

/// The providers that know whether a tile of each kind is loaded.
#[derive(Default)]
pub struct ViewTileSources {
    vector: Vec<Box<dyn HasTile>>,
    raster: Vec<Box<dyn HasTile>>,
}

impl ViewTileSources {
    /// Registers a provider for one kind of tile.
    pub fn add<H: HasTile + 'static + Default>(&mut self, kind: TileKind) -> &mut Self {
        self.items_mut(kind).push(Box::<H>::default());
        self
    }

    /// Registers a resource-backed provider for one kind of tile.
    pub fn add_resource_query<Q: ResourceQuery + 'static>(&mut self, kind: TileKind) -> &mut Self
    where
        for<'a> Q::Item<'a>: HasTile,
    {
        self.items_mut(kind)
            .push(Box::new(QueryHasTile::<Q>::default()));
        self
    }

    /// Forgets every provider, so every tile counts as loaded.
    pub fn clear(&mut self) {
        self.vector.clear();
        self.raster.clear();
    }

    /// The providers of one kind, answering as one: a tile is loaded when every provider of
    /// the kind has it, and everything counts as loaded while no provider is registered.
    pub fn of_kind(&self, kind: TileKind) -> KindSources<'_> {
        KindSources(match kind {
            TileKind::Vector => &self.vector,
            TileKind::Raster => &self.raster,
        })
    }

    fn items_mut(&mut self, kind: TileKind) -> &mut Vec<Box<dyn HasTile>> {
        match kind {
            TileKind::Vector => &mut self.vector,
            TileKind::Raster => &mut self.raster,
        }
    }
}

/// The providers of one tile kind.
pub struct KindSources<'a>(&'a [Box<dyn HasTile>]);

impl HasTile for KindSources<'_> {
    fn has_tile(&self, coords: WorldTileCoords, world: &World) -> bool {
        self.0.iter().all(|item| item.has_tile(coords, world))
    }
    fn has_source_tile(
        &self,
        source: &RasterSourceId,
        coords: WorldTileCoords,
        world: &World,
    ) -> bool {
        self.0
            .iter()
            .all(|item| item.has_source_tile(source, coords, world))
    }
    fn nearest_ancestor_candidate(
        &self,
        coords: WorldTileCoords,
        world: &World,
    ) -> Option<WorldTileCoords> {
        let Some((first, rest)) = self.0.split_first() else {
            return coords.get_parent();
        };
        let mut best = first.nearest_ancestor_candidate(coords, world);
        for item in rest {
            best = coarser_ancestor(best, item.nearest_ancestor_candidate(coords, world));
            best?;
        }
        best
    }
    fn nearest_source_ancestor_candidate(
        &self,
        source: &RasterSourceId,
        coords: WorldTileCoords,
        world: &World,
    ) -> Option<WorldTileCoords> {
        let Some((first, rest)) = self.0.split_first() else {
            return coords.get_parent();
        };
        let mut best = first.nearest_source_ancestor_candidate(source, coords, world);
        for item in rest {
            best = coarser_ancestor(
                best,
                item.nearest_source_ancestor_candidate(source, coords, world),
            );
            best?;
        }
        best
    }
    fn has_descendant_candidates(
        &self,
        coords: WorldTileCoords,
        max_depth: usize,
        world: &World,
    ) -> bool {
        if self.0.is_empty() {
            return max_depth > 0;
        }
        self.0
            .iter()
            .all(|item| item.has_descendant_candidates(coords, max_depth, world))
    }
    fn has_source_descendant_candidates(
        &self,
        source: &RasterSourceId,
        coords: WorldTileCoords,
        max_depth: usize,
        world: &World,
    ) -> bool {
        if self.0.is_empty() {
            return max_depth > 0;
        }
        self.0
            .iter()
            .all(|item| item.has_source_descendant_candidates(source, coords, max_depth, world))
    }
}

pub(crate) struct SourceTiles<'a, H> {
    pub(crate) source: &'a RasterSourceId,
    pub(crate) availability: H,
}

impl<H: HasTile> HasTile for SourceTiles<'_, H> {
    fn has_tile(&self, coords: WorldTileCoords, world: &World) -> bool {
        self.availability
            .has_source_tile(self.source, coords, world)
    }
    fn nearest_ancestor_candidate(
        &self,
        coords: WorldTileCoords,
        world: &World,
    ) -> Option<WorldTileCoords> {
        self.availability
            .nearest_source_ancestor_candidate(self.source, coords, world)
    }
    fn has_descendant_candidates(
        &self,
        coords: WorldTileCoords,
        max_depth: usize,
        world: &World,
    ) -> bool {
        self.availability
            .has_source_descendant_candidates(self.source, coords, max_depth, world)
    }
}
