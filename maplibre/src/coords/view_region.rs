//! Visible tile regions across canonical and wrapped worlds.

use std::collections::HashSet;

use crate::{
    coords::{WorldCoords, WorldTileCoords, Zoom, ZoomLevel},
    util::math::Aabb2,
};

/// Defines a bounding box on a tiled map with a [`ZoomLevel`] and a padding.
#[derive(Clone, Debug)]
pub struct ViewRegion {
    min_tile: WorldTileCoords,
    max_tile: WorldTileCoords,
    explicit_tiles: Option<Vec<WorldTileCoords>>,
    /// At which zoom level does this region exist
    zoom_level: ZoomLevel,
    /// Padding around this view region
    padding: i32,
    /// The maximum amount of tiles this view region contains
    max_n_tiles: usize,
}

impl ViewRegion {
    /// Covers a world-pixel box at `zoom` with tiles at `z` and an integer tile padding.
    /// `max_n_tiles` limits iteration, not the membership test in [`Self::is_in_view`].
    pub fn new(
        view_region: Aabb2<f64>,
        padding: i32,
        max_n_tiles: usize,
        zoom: Zoom,
        z: ZoomLevel,
    ) -> Self {
        let min_world: WorldCoords = WorldCoords::at_ground(view_region.min.x, view_region.min.y);
        let min_world_tile: WorldTileCoords = min_world.into_world_tile(z, zoom);
        let max_world: WorldCoords = WorldCoords::at_ground(view_region.max.x, view_region.max.y);
        let max_world_tile: WorldTileCoords = max_world.into_world_tile(z, zoom);

        Self {
            min_tile: min_world_tile,
            max_tile: max_world_tile,
            explicit_tiles: None,
            zoom_level: z,
            max_n_tiles,
            padding,
        }
    }

    /// Creates a region from an explicit projection-aware tile selection.
    pub fn from_tiles(
        tiles: Vec<WorldTileCoords>,
        zoom_level: ZoomLevel,
        max_n_tiles: usize,
    ) -> Self {
        Self {
            min_tile: WorldTileCoords::default(),
            max_tile: WorldTileCoords::default(),
            explicit_tiles: Some(tiles),
            zoom_level,
            max_n_tiles,
            padding: 0,
        }
    }

    /// Returns the grid level supplied when constructing this region.
    pub fn zoom_level(&self) -> ZoomLevel {
        self.zoom_level
    }

    /// Tests the padded bounds or explicit selection without applying the iteration limit.
    pub fn is_in_view(&self, &world_coords: &WorldTileCoords) -> bool {
        if let Some(tiles) = &self.explicit_tiles {
            return tiles.contains(&world_coords);
        }
        world_coords.x <= self.max_tile.x + self.padding
            && world_coords.y <= self.max_tile.y + self.padding
            && world_coords.x >= self.min_tile.x - self.padding
            && world_coords.y >= self.min_tile.y - self.padding
            && world_coords.z == self.zoom_level
    }

    /// The tiles a view sees, with those seen in copies of the world as the tiles they repeat,
    /// each once: up to the configured limit, in explicit selection order or column-major
    /// grid order.
    pub fn iter(&self) -> Box<dyn Iterator<Item = WorldTileCoords> + '_> {
        let mut seen = HashSet::new();
        Box::new(
            self.copies()
                .map(|coords| coords.wrapped().map_or(coords, |(tile, _)| tile))
                .filter(move |tile| seen.insert(*tile)),
        )
    }

    /// The tiles of the view as drawn, a tile seen again in another copy of the world once more
    /// at coordinates beyond the grid.
    pub fn copies(&self) -> Box<dyn Iterator<Item = WorldTileCoords> + '_> {
        if let Some(tiles) = &self.explicit_tiles {
            return Box::new(tiles.iter().copied().take(self.max_n_tiles));
        }
        Box::new(
            (self.min_tile.x - self.padding..self.max_tile.x + 1 + self.padding)
                .flat_map(move |x| {
                    (self.min_tile.y - self.padding..self.max_tile.y + 1 + self.padding)
                        .map(move |y| (x, y, self.zoom_level).into())
                })
                .take(self.max_n_tiles),
        )
    }
}
