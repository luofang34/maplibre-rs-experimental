//! Which DEM tile stands behind each rendered terrain tile, and the elevations they span.
//!
//! Mirrors GL JS `TerrainCoverageIndex`: the index is rebuilt every frame from the tiles the
//! terrain draws, so elevation queries, ray casts and tile culling all see the same surface.

use std::collections::HashMap;

use crate::{
    context::MapContext,
    coords::{TileCoords, WorldTileCoords, ZoomLevel, EXTENT},
    projection::globe::covering::{TileElevationProvider, TileElevationRange},
    render::{
        projection::view_region_for_projection, tile_view_pattern::DEFAULT_TILE_SIZE,
        view_state::ViewStatePadding,
    },
    tcs::{system::SystemResult, tiles::Tiles},
    terrain::{
        request_system::dem_tile_coords,
        source::{dem_source, DemSource},
        DemTileComponent,
    },
};

/// Metres added around the sampled minimum and maximum so ray casts bracket the surface.
const BRACKET_PADDING_METERS: f64 = 10.0;
/// Largest tile-local coordinate, keeping samples inside the tile that contains them.
const MAX_TILE_COORD: f64 = EXTENT * (1.0 - 1e-12);

/// Result of sampling the terrain surface at one position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainSample {
    /// Whether a rendered tile contains the position.
    pub covered: bool,
    /// Whether that tile's DEM has loaded, so the elevation is real rather than sea level.
    pub dem_loaded: bool,
    /// Exaggerated elevation in metres, zero while not loaded.
    pub elevation: f64,
}

impl TerrainSample {
    const NOT_COVERED: Self = Self {
        covered: false,
        dem_loaded: false,
        elevation: 0.0,
    };
}

/// Elevation index of the tiles rendered this frame.
#[derive(Clone, Debug, Default)]
pub struct TerrainCoverageIndex {
    /// Zoom levels of the rendered tiles, highest first.
    zooms: Vec<u8>,
    /// Rendered tile to the DEM tile it samples, `None` while that DEM is still loading.
    rendered: HashMap<WorldTileCoords, Option<WorldTileCoords>>,
    /// Interior elevation bounds of every loaded DEM tile, without exaggeration.
    dem_bounds: HashMap<WorldTileCoords, (f64, f64)>,
    min_elevation: f64,
    max_elevation: f64,
    exaggeration: f64,
    minzoom: u8,
    maxzoom: u8,
}

impl TerrainCoverageIndex {
    /// Indexes the rendered tiles against the DEM tiles loaded in `tiles`.
    pub fn build(
        rendered: impl IntoIterator<Item = WorldTileCoords>,
        tiles: &Tiles,
        dem: &DemSource,
    ) -> Self {
        let mut index = Self {
            exaggeration: f64::from(dem.exaggeration),
            minzoom: dem.minzoom,
            maxzoom: dem.maxzoom,
            ..Self::default()
        };
        for tile in tiles.tiles.values() {
            if let Some(DemTileComponent::Loaded(dem)) =
                tiles.query::<&DemTileComponent>(tile.coords)
            {
                index
                    .dem_bounds
                    .insert(tile.coords, (dem.tile.min(), dem.tile.max()));
            }
        }
        let (mut min, mut max) = (0.0_f64, 0.0_f64);
        for coords in rendered {
            let zoom = u8::from(coords.z);
            if !index.zooms.contains(&zoom) {
                index.zooms.push(zoom);
            }
            let source = index.loaded_dem_for(coords);
            if let Some((tile_min, tile_max)) = source.and_then(|dem| index.dem_bounds.get(&dem)) {
                min = min.min(tile_min * index.exaggeration);
                max = max.max(tile_max * index.exaggeration);
            }
            index.rendered.insert(coords, source);
        }
        index.zooms.sort_unstable_by(|a, b| b.cmp(a));
        index.min_elevation = min - BRACKET_PADDING_METERS;
        index.max_elevation = max + BRACKET_PADDING_METERS;
        index
    }

    /// Whether any tile is rendered.
    pub fn is_empty(&self) -> bool {
        self.rendered.is_empty()
    }

    /// Lowest exaggerated elevation among the rendered tiles, minus the bracket padding.
    pub fn min_elevation(&self) -> f64 {
        self.min_elevation
    }

    /// Highest exaggerated elevation among the rendered tiles, plus the bracket padding.
    pub fn max_elevation(&self) -> f64 {
        self.max_elevation
    }

    /// Elevation multiplier of the terrain.
    pub fn exaggeration(&self) -> f64 {
        self.exaggeration
    }

    /// The loaded DEM tile that stands in for `coords`: its own DEM tile or the nearest ancestor.
    pub fn loaded_dem_for(&self, coords: WorldTileCoords) -> Option<WorldTileCoords> {
        let mut current = dem_tile_coords(coords, self.minzoom, self.maxzoom)?;
        loop {
            if self.dem_bounds.contains_key(&current) {
                return Some(current);
            }
            current = current.get_parent()?;
        }
    }

    /// Exaggerated elevation bounds of a tile, from the loaded DEM that covers it.
    pub fn tile_elevation_range(&self, coords: WorldTileCoords) -> Option<TileElevationRange> {
        let (min, max) = self
            .loaded_dem_for(coords)
            .and_then(|dem| self.dem_bounds.get(&dem))?;
        Some(TileElevationRange {
            min_meters: min * self.exaggeration,
            max_meters: max * self.exaggeration,
        })
    }

    /// Samples the rendered surface at Mercator coordinates in `0..1`.
    pub fn sample(&self, tiles: &Tiles, mercator_x: f64, mercator_y: f64) -> TerrainSample {
        if !(0.0..1.0).contains(&mercator_y) {
            return TerrainSample::NOT_COVERED;
        }
        let wrapped_x = mercator_x - mercator_x.floor();
        for &zoom in &self.zooms {
            let scale = 2_f64.powi(i32::from(zoom));
            let coords = WorldTileCoords {
                x: (wrapped_x * scale).floor() as i32,
                y: (mercator_y * scale).floor() as i32,
                z: ZoomLevel::new(zoom),
            };
            let Some(source) = self.rendered.get(&coords) else {
                continue;
            };
            let Some(dem_coords) = source else {
                return TerrainSample {
                    covered: true,
                    dem_loaded: false,
                    elevation: 0.0,
                };
            };
            let Some(DemTileComponent::Loaded(dem)) = tiles.query::<&DemTileComponent>(*dem_coords)
            else {
                return TerrainSample {
                    covered: true,
                    dem_loaded: false,
                    elevation: 0.0,
                };
            };
            let dem_scale = 2_f64.powi(i32::from(u8::from(dem_coords.z)));
            let x =
                ((wrapped_x * dem_scale - f64::from(dem_coords.x)) * EXTENT).min(MAX_TILE_COORD);
            let y =
                ((mercator_y * dem_scale - f64::from(dem_coords.y)) * EXTENT).min(MAX_TILE_COORD);
            return TerrainSample {
                covered: true,
                dem_loaded: true,
                elevation: dem.tile.elevation_at_tile_coords(x, y) * self.exaggeration,
            };
        }
        TerrainSample::NOT_COVERED
    }

    /// Exaggerated elevation at Mercator coordinates, or `None` where no DEM has loaded.
    pub fn elevation_at(&self, tiles: &Tiles, mercator_x: f64, mercator_y: f64) -> Option<f64> {
        let sample = self.sample(tiles, mercator_x, mercator_y);
        sample.dem_loaded.then_some(sample.elevation)
    }

    /// Finest cached elevation, including terrain outside the current view, for camera clearance.
    pub fn elevation_cached(&self, tiles: &Tiles, x: f64, y: f64) -> Option<f64> {
        self.elevation_at_zoom(tiles, x, y, self.maxzoom.saturating_add(1).min(24))
    }

    /// Exaggerated elevation at Mercator coordinates from any loaded DEM, rendered or not, as
    /// the tile at `zoom` would sample it; `None` where nothing covering it has loaded.
    pub fn elevation_at_zoom(
        &self,
        tiles: &Tiles,
        mercator_x: f64,
        mercator_y: f64,
        zoom: u8,
    ) -> Option<f64> {
        if !(0.0..1.0).contains(&mercator_y) {
            return None;
        }
        let wrapped_x = mercator_x - mercator_x.floor();
        let scale = 2_f64.powi(i32::from(zoom));
        let coords = WorldTileCoords {
            x: (wrapped_x * scale).floor() as i32,
            y: (mercator_y * scale).floor() as i32,
            z: ZoomLevel::new(zoom),
        };
        let dem_coords = self.loaded_dem_for(coords)?;
        let DemTileComponent::Loaded(dem) = tiles.query::<&DemTileComponent>(dem_coords)? else {
            return None;
        };
        let dem_scale = 2_f64.powi(i32::from(u8::from(dem_coords.z)));
        let x = ((wrapped_x * dem_scale - f64::from(dem_coords.x)) * EXTENT).min(MAX_TILE_COORD);
        let y = ((mercator_y * dem_scale - f64::from(dem_coords.y)) * EXTENT).min(MAX_TILE_COORD);
        Some(dem.tile.elevation_at_tile_coords(x, y) * self.exaggeration)
    }
}

/// Per-tile culling bounds from the index, with a wide fallback where no DEM has loaded yet.
pub struct IndexedTileElevation<'a> {
    /// Index providing bounds for tiles whose DEM, or an ancestor's, has loaded.
    pub index: &'a TerrainCoverageIndex,
    /// Bounds assumed for tiles without any loaded DEM.
    pub fallback: TileElevationRange,
}

impl TileElevationProvider for IndexedTileElevation<'_> {
    fn elevation_range(&self, tile: TileCoords) -> TileElevationRange {
        let coords = WorldTileCoords {
            x: tile.x as i32,
            y: tile.y as i32,
            z: tile.z,
        };
        self.index
            .tile_elevation_range(coords)
            .unwrap_or(self.fallback)
    }
}

/// Rebuilds the coverage index from the tiles the terrain will draw this frame.
pub fn coverage_system(
    MapContext {
        style,
        view_state,
        world,
        ..
    }: &mut MapContext,
) -> SystemResult {
    let Some(dem) = dem_source(style) else {
        world.resources.insert(TerrainCoverageIndex::default());
        return Ok(());
    };
    let rendered = view_region_for_projection(
        style,
        view_state,
        world,
        view_state.zoom().zoom_level(DEFAULT_TILE_SIZE),
        ViewStatePadding::Tight,
    )
    .map_err(|error| {
        tracing::error!(%error, "unable to select terrain coverage tiles");
        crate::tcs::system::SystemError::Setup
    })?;
    let index = match rendered {
        Some(region) => TerrainCoverageIndex::build(
            region
                .iter()
                .filter(|coords| coords.build_quad_key().is_some()),
            &world.tiles,
            &dem,
        ),
        None => TerrainCoverageIndex::default(),
    };
    world.resources.insert(index);
    Ok(())
}

#[cfg(test)]
mod tests;
