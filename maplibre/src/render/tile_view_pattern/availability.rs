//! Pyramid-accelerated tile availability traits and combinators.

mod sources;

pub(crate) use sources::SourceTiles;
pub use sources::{KindSources, QueryHasTile, ViewTileSources};

use crate::{
    coords::{WorldTileCoords, MAX_ZOOM},
    io::tile_sources::RasterSourceId,
    tcs::{pyramid::TilePyramid, world::World},
};

/// Availability provider used to choose loaded replacements for requested tiles.
/// Missing or uninitialized backing resources must report tiles as unavailable.
pub trait HasTile {
    /// Whether this tile has the resources required by the provider to render.
    fn has_tile(&self, coords: WorldTileCoords, world: &World) -> bool;

    /// Checks one raster source; providers without source-specific state apply their ordinary constraint.
    fn has_source_tile(
        &self,
        source: &RasterSourceId,
        coords: WorldTileCoords,
        world: &World,
    ) -> bool {
        let _ = source;
        self.has_tile(coords, world)
    }

    /// Deepest candidate strictly coarser than `coords` that could exist in this provider.
    /// Providers backed by a [`crate::tcs::pyramid::TilePyramid`] jump over empty zoom levels
    /// in `O(1)`; unindexed providers step one level up via [`WorldTileCoords::get_parent`].
    fn nearest_ancestor_candidate(
        &self,
        coords: WorldTileCoords,
        world: &World,
    ) -> Option<WorldTileCoords> {
        let _ = world;
        coords.get_parent()
    }

    /// Per-raster-source counterpart to [`Self::nearest_ancestor_candidate`].
    fn nearest_source_ancestor_candidate(
        &self,
        source: &RasterSourceId,
        coords: WorldTileCoords,
        world: &World,
    ) -> Option<WorldTileCoords> {
        let _ = source;
        self.nearest_ancestor_candidate(coords, world)
    }

    /// Whether any descendant of `coords` within `1..=max_depth` levels could exist in this
    /// provider. Pyramid-backed providers prune empty subtrees in `O(1)` without probing `4^d`
    /// missing child coordinates.
    fn has_descendant_candidates(
        &self,
        coords: WorldTileCoords,
        max_depth: usize,
        world: &World,
    ) -> bool {
        let _ = (coords, world);
        max_depth > 0
    }

    /// Per-raster-source counterpart to [`Self::has_descendant_candidates`].
    fn has_source_descendant_candidates(
        &self,
        source: &RasterSourceId,
        coords: WorldTileCoords,
        max_depth: usize,
        world: &World,
    ) -> bool {
        let _ = source;
        self.has_descendant_candidates(coords, max_depth, world)
    }

    /// Finds the nearest available ancestor, including `coords` itself; returns `None` at an empty root.
    fn get_available_parent(
        &self,
        coords: WorldTileCoords,
        world: &World,
    ) -> Option<WorldTileCoords> {
        let mut current = coords;
        loop {
            if self.has_tile(current, world) {
                return Some(current);
            }
            current = self.nearest_ancestor_candidate(current, world)?;
        }
    }

    /// Loaded descendants that cover `coords` completely, at most `search_depth` levels down.
    ///
    /// Returns `None` at depth zero or if any quadrant lacks data within the search depth.
    /// Coordinates must allow subdivision through the requested depth without integer overflow.
    fn get_complete_children(
        &self,
        coords: WorldTileCoords,
        world: &World,
        search_depth: usize,
    ) -> Option<Vec<WorldTileCoords>> {
        if search_depth == 0
            || usize::from(u8::from(coords.z)) + 1 >= MAX_ZOOM
            || !self.has_descendant_candidates(coords, search_depth, world)
        {
            return None;
        }
        let mut output = Vec::with_capacity(4);
        for child in coords.get_children() {
            if self.has_tile(child, world) {
                output.push(child);
            } else {
                output.extend(self.get_complete_children(child, world, search_depth - 1)?);
            }
        }
        Some(output)
    }

    /// Finds available descendants up to `search_depth` levels below `coords`.
    /// Stops descending each loaded branch; coverage can be partial and an empty result is `Some([])`.
    /// Coordinates must allow subdivision through the requested depth without integer overflow.
    fn get_available_children(
        &self,
        coords: WorldTileCoords,
        world: &World,
        search_depth: usize,
    ) -> Option<Vec<WorldTileCoords>> {
        if search_depth == 0
            || usize::from(u8::from(coords.z)) + 1 >= MAX_ZOOM
            || !self.has_descendant_candidates(coords, search_depth, world)
        {
            return Some(Vec::new());
        }
        let mut children = coords.get_children().to_vec();
        let mut output = Vec::new();

        for step in 0..search_depth {
            let remaining = search_depth - 1 - step;
            let mut new_children = Vec::new();

            for child in children {
                if self.has_tile(child, world) {
                    output.push(child);
                } else if remaining > 0
                    && usize::from(u8::from(child.z)) + 1 < MAX_ZOOM
                    && self.has_descendant_candidates(child, remaining, world)
                {
                    new_children.extend(child.get_children());
                }
            }

            if new_children.is_empty() {
                break;
            }
            children = new_children;
        }

        Some(output)
    }
}

#[inline]
pub(super) fn coarser_ancestor(
    a: Option<WorldTileCoords>,
    b: Option<WorldTileCoords>,
) -> Option<WorldTileCoords> {
    let (a, b) = (a?, b?);
    Some(if a.z <= b.z { a } else { b })
}

impl<T> HasTile for TilePyramid<T> {
    fn has_tile(&self, coords: WorldTileCoords, _world: &World) -> bool {
        self.contains(coords)
    }
    fn nearest_ancestor_candidate(
        &self,
        coords: WorldTileCoords,
        _world: &World,
    ) -> Option<WorldTileCoords> {
        self.nearest_ancestor(coords).map(|(ancestor, _)| ancestor)
    }
    fn has_descendant_candidates(
        &self,
        coords: WorldTileCoords,
        max_depth: usize,
        _world: &World,
    ) -> bool {
        self.has_descendants(coords, max_depth)
    }
}

impl<A: HasTile> HasTile for &A {
    fn has_tile(&self, coords: WorldTileCoords, world: &World) -> bool {
        A::has_tile(*self, coords, world)
    }
    fn has_source_tile(
        &self,
        source: &RasterSourceId,
        coords: WorldTileCoords,
        world: &World,
    ) -> bool {
        A::has_source_tile(*self, source, coords, world)
    }
    fn nearest_ancestor_candidate(
        &self,
        coords: WorldTileCoords,
        world: &World,
    ) -> Option<WorldTileCoords> {
        A::nearest_ancestor_candidate(*self, coords, world)
    }
    fn nearest_source_ancestor_candidate(
        &self,
        source: &RasterSourceId,
        coords: WorldTileCoords,
        world: &World,
    ) -> Option<WorldTileCoords> {
        A::nearest_source_ancestor_candidate(*self, source, coords, world)
    }
    fn has_descendant_candidates(
        &self,
        coords: WorldTileCoords,
        max_depth: usize,
        world: &World,
    ) -> bool {
        A::has_descendant_candidates(*self, coords, max_depth, world)
    }
    fn has_source_descendant_candidates(
        &self,
        source: &RasterSourceId,
        coords: WorldTileCoords,
        max_depth: usize,
        world: &World,
    ) -> bool {
        A::has_source_descendant_candidates(*self, source, coords, max_depth, world)
    }
}

impl<A: HasTile> HasTile for (A,) {
    fn has_tile(&self, coords: WorldTileCoords, world: &World) -> bool {
        self.0.has_tile(coords, world)
    }
    fn has_source_tile(
        &self,
        source: &RasterSourceId,
        coords: WorldTileCoords,
        world: &World,
    ) -> bool {
        self.0.has_source_tile(source, coords, world)
    }
    fn nearest_ancestor_candidate(
        &self,
        coords: WorldTileCoords,
        world: &World,
    ) -> Option<WorldTileCoords> {
        self.0.nearest_ancestor_candidate(coords, world)
    }
    fn nearest_source_ancestor_candidate(
        &self,
        source: &RasterSourceId,
        coords: WorldTileCoords,
        world: &World,
    ) -> Option<WorldTileCoords> {
        self.0
            .nearest_source_ancestor_candidate(source, coords, world)
    }
    fn has_descendant_candidates(
        &self,
        coords: WorldTileCoords,
        max_depth: usize,
        world: &World,
    ) -> bool {
        self.0.has_descendant_candidates(coords, max_depth, world)
    }
    fn has_source_descendant_candidates(
        &self,
        source: &RasterSourceId,
        coords: WorldTileCoords,
        max_depth: usize,
        world: &World,
    ) -> bool {
        self.0
            .has_source_descendant_candidates(source, coords, max_depth, world)
    }
}

impl<A: HasTile, B: HasTile> HasTile for (A, B) {
    fn has_tile(&self, coords: WorldTileCoords, world: &World) -> bool {
        self.0.has_tile(coords, world) && self.1.has_tile(coords, world)
    }
    fn has_source_tile(
        &self,
        source: &RasterSourceId,
        coords: WorldTileCoords,
        world: &World,
    ) -> bool {
        self.0.has_source_tile(source, coords, world)
            && self.1.has_source_tile(source, coords, world)
    }
    fn nearest_ancestor_candidate(
        &self,
        coords: WorldTileCoords,
        world: &World,
    ) -> Option<WorldTileCoords> {
        coarser_ancestor(
            self.0.nearest_ancestor_candidate(coords, world),
            self.1.nearest_ancestor_candidate(coords, world),
        )
    }
    fn nearest_source_ancestor_candidate(
        &self,
        source: &RasterSourceId,
        coords: WorldTileCoords,
        world: &World,
    ) -> Option<WorldTileCoords> {
        coarser_ancestor(
            self.0
                .nearest_source_ancestor_candidate(source, coords, world),
            self.1
                .nearest_source_ancestor_candidate(source, coords, world),
        )
    }
    fn has_descendant_candidates(
        &self,
        coords: WorldTileCoords,
        max_depth: usize,
        world: &World,
    ) -> bool {
        self.0.has_descendant_candidates(coords, max_depth, world)
            && self.1.has_descendant_candidates(coords, max_depth, world)
    }
    fn has_source_descendant_candidates(
        &self,
        source: &RasterSourceId,
        coords: WorldTileCoords,
        max_depth: usize,
        world: &World,
    ) -> bool {
        self.0
            .has_source_descendant_candidates(source, coords, max_depth, world)
            && self
                .1
                .has_source_descendant_candidates(source, coords, max_depth, world)
    }
}

impl<A: HasTile, B: HasTile, C: HasTile> HasTile for (A, B, C) {
    fn has_tile(&self, coords: WorldTileCoords, world: &World) -> bool {
        self.0.has_tile(coords, world)
            && self.1.has_tile(coords, world)
            && self.2.has_tile(coords, world)
    }
    fn has_source_tile(
        &self,
        source: &RasterSourceId,
        coords: WorldTileCoords,
        world: &World,
    ) -> bool {
        self.0.has_source_tile(source, coords, world)
            && self.1.has_source_tile(source, coords, world)
            && self.2.has_source_tile(source, coords, world)
    }
    fn nearest_ancestor_candidate(
        &self,
        coords: WorldTileCoords,
        world: &World,
    ) -> Option<WorldTileCoords> {
        coarser_ancestor(
            coarser_ancestor(
                self.0.nearest_ancestor_candidate(coords, world),
                self.1.nearest_ancestor_candidate(coords, world),
            ),
            self.2.nearest_ancestor_candidate(coords, world),
        )
    }
    fn nearest_source_ancestor_candidate(
        &self,
        source: &RasterSourceId,
        coords: WorldTileCoords,
        world: &World,
    ) -> Option<WorldTileCoords> {
        coarser_ancestor(
            coarser_ancestor(
                self.0
                    .nearest_source_ancestor_candidate(source, coords, world),
                self.1
                    .nearest_source_ancestor_candidate(source, coords, world),
            ),
            self.2
                .nearest_source_ancestor_candidate(source, coords, world),
        )
    }
    fn has_descendant_candidates(
        &self,
        coords: WorldTileCoords,
        max_depth: usize,
        world: &World,
    ) -> bool {
        self.0.has_descendant_candidates(coords, max_depth, world)
            && self.1.has_descendant_candidates(coords, max_depth, world)
            && self.2.has_descendant_candidates(coords, max_depth, world)
    }
    fn has_source_descendant_candidates(
        &self,
        source: &RasterSourceId,
        coords: WorldTileCoords,
        max_depth: usize,
        world: &World,
    ) -> bool {
        self.0
            .has_source_descendant_candidates(source, coords, max_depth, world)
            && self
                .1
                .has_source_descendant_candidates(source, coords, max_depth, world)
            && self
                .2
                .has_source_descendant_candidates(source, coords, max_depth, world)
    }
}
