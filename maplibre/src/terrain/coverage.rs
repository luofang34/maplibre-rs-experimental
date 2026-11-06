//! Which DEM tile stands behind each rendered terrain tile, and the elevations they span.
//!
//! Mirrors GL JS `TerrainCoverageIndex`: the index is rebuilt every frame from the tiles the
//! terrain draws, so elevation queries, ray casts and tile culling all see the same surface.
//!
//! The ground at a point is asked one of two ways. The drawn ground, the triangles the frame
//! draws, is [`DrawnTerrain::ground_at`](crate::terrain::sightline::DrawnTerrain::ground_at):
//! it decides lines of sight, picking and conversion between the screen and the ground. The
//! best loaded DEM, [`TerrainCoverageIndex::loaded_ground_cached`], reads whatever DEM has
//! loaded, inside the view or not: it keeps the camera and the center above the terrain.
//! Between them sits the crate's stitched DEM sample, the height the drawn mesh's vertices
//! take, which the mesh and the flat camera's gestures are built from.

use std::{collections::HashMap, sync::Arc};

use crate::{
    context::MapContext,
    coords::{TileCoords, WorldTileCoords, ZoomLevel, EXTENT},
    projection::globe::covering::{TileElevationProvider, TileElevationRange},
    render::view_state::ViewStatePadding,
    tcs::{system::SystemResult, tiles::Tiles},
    terrain::{
        request_system::dem_tile_coords,
        source::{dem_source, DemSource},
        DemTileComponent,
    },
};

/// Metres added around the sampled minimum and maximum so ray casts bracket the surface.
const BRACKET_PADDING_METERS: f64 = 10.0;
/// How finely [`TerrainCoverageIndex::loaded_ground_cached`] reads the loaded DEM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DemDetail {
    /// As the terrain tile at this zoom samples it: its DEM tile, or the nearest loaded
    /// ancestor, as GL JS lifts its center by the tile at the integer zoom.
    Tile(u8),
    /// The finest DEM loaded, down to the source's maxzoom.
    Finest,
}

/// Largest tile-local coordinate, keeping samples inside the tile that contains them.
const MAX_TILE_COORD: f64 = EXTENT * (1.0 - 1e-12);

/// Result of sampling the stitched DEM of the rendered tiles at one position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TerrainSample {
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
    /// Samples along a DEM tile's side.
    dem_tile_size: u32,
    poles: super::PolarPoleElevations,
    edges: Arc<HashMap<WorldTileCoords, super::queue_system::edges::EdgeHeights>>,
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
            dem_tile_size: dem.tile_size,
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

    /// Configured polar cap datum elevations.
    pub fn poles(&self) -> super::PolarPoleElevations {
        self.poles
    }

    /// Configures the north and south polar cap datum elevations, updating rendered bounds.
    pub fn with_poles(mut self, poles: super::PolarPoleElevations) -> Self {
        self.poles = poles;
        if self.rendered.is_empty() {
            return self;
        }
        let mut min = self.min_elevation + BRACKET_PADDING_METERS;
        let mut max = self.max_elevation - BRACKET_PADDING_METERS;
        for coords in self.rendered.keys() {
            let max_row = 1_i32
                .checked_shl(u32::from(u8::from(coords.z)))
                .unwrap_or(1)
                - 1;
            if coords.y == 0 {
                let elev = f64::from(poles.north_meters) * self.exaggeration;
                min = min.min(elev);
                max = max.max(elev);
            }
            if coords.y == max_row {
                let elev = f64::from(poles.south_meters) * self.exaggeration;
                min = min.min(elev);
                max = max.max(elev);
            }
        }
        self.min_elevation = min - BRACKET_PADDING_METERS;
        self.max_elevation = max + BRACKET_PADDING_METERS;
        self
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
        let mut min_meters = min * self.exaggeration;
        let mut max_meters = max * self.exaggeration;
        let max_row = 1_i32
            .checked_shl(u32::from(u8::from(coords.z)))
            .unwrap_or(1)
            - 1;
        if coords.y == 0 && self.poles.north_meters != 0.0 {
            let pole = f64::from(self.poles.north_meters) * self.exaggeration;
            min_meters = min_meters.min(pole);
            max_meters = max_meters.max(pole);
        }
        if coords.y == max_row && self.poles.south_meters != 0.0 {
            let pole = f64::from(self.poles.south_meters) * self.exaggeration;
            min_meters = min_meters.min(pole);
            max_meters = max_meters.max(pole);
        }
        Some(TileElevationRange {
            min_meters,
            max_meters,
        })
    }

    /// The rendered tiles.
    pub fn rendered_tiles(&self) -> impl Iterator<Item = WorldTileCoords> + '_ {
        self.rendered.keys().copied()
    }

    /// Zoom of the finest rendered tiles.
    pub fn finest_zoom(&self) -> Option<u8> {
        self.zooms.first().copied()
    }

    /// Samples along a DEM tile's side, the finest detail the terrain can have per tile.
    pub fn dem_tile_size(&self) -> u32 {
        self.dem_tile_size.max(1)
    }

    /// The rendered tile containing Mercator coordinates in `0..1`, finest first, as the
    /// stitched sample picks it.
    pub fn rendered_tile_at(&self, mercator_x: f64, mercator_y: f64) -> Option<WorldTileCoords> {
        if !(0.0..1.0).contains(&mercator_y) {
            return None;
        }
        let wrapped_x = mercator_x - mercator_x.floor();
        self.zooms.iter().find_map(|&zoom| {
            let scale = 2_f64.powi(i32::from(zoom));
            let coords = WorldTileCoords {
                x: (wrapped_x * scale).floor() as i32,
                y: (mercator_y * scale).floor() as i32,
                z: ZoomLevel::new(zoom),
            };
            self.rendered.contains_key(&coords).then_some(coords)
        })
    }

    /// Samples the stitched DEM of the rendered tile containing Mercator coordinates in `0..1`.
    pub(crate) fn sample(&self, tiles: &Tiles, mercator_x: f64, mercator_y: f64) -> TerrainSample {
        if !(0.0..1.0).contains(&mercator_y) {
            return TerrainSample::NOT_COVERED;
        }
        self.rendered_tile_at(mercator_x, mercator_y)
            .map_or(TerrainSample::NOT_COVERED, |coords| {
                self.sample_in(tiles, coords, mercator_x, mercator_y)
            })
    }

    /// Samples the DEM behind the rendered tile `coords` at Mercator coordinates in `0..1`, on
    /// or inside its edges: bilinear within the DEM tile, stitched to the neighbours' heights
    /// along the tile's edges. The drawn mesh takes these heights at its vertices and is flat
    /// between them, so away from a vertex the drawn ground can differ from this sample.
    pub(crate) fn sample_in(
        &self,
        tiles: &Tiles,
        coords: WorldTileCoords,
        mercator_x: f64,
        mercator_y: f64,
    ) -> TerrainSample {
        let not_loaded = TerrainSample {
            covered: true,
            dem_loaded: false,
            elevation: 0.0,
        };
        let Some(source) = self.rendered.get(&coords) else {
            return TerrainSample::NOT_COVERED;
        };
        let Some(dem_coords) = source else {
            return not_loaded;
        };
        let Some(DemTileComponent::Loaded(dem)) = tiles.query::<&DemTileComponent>(*dem_coords)
        else {
            return not_loaded;
        };
        // The world copy of `mercator_x` nearest the tile, so its east edge stays on the tile.
        let west = f64::from(coords.x) / 2_f64.powi(i32::from(u8::from(coords.z)));
        let wrapped_x = mercator_x - (mercator_x - west + 0.5).floor();
        let dem_scale = 2_f64.powi(i32::from(u8::from(dem_coords.z)));
        let x = ((wrapped_x * dem_scale - f64::from(dem_coords.x)) * EXTENT).min(MAX_TILE_COORD);
        let y = ((mercator_y * dem_scale - f64::from(dem_coords.y)) * EXTENT).min(MAX_TILE_COORD);
        TerrainSample {
            covered: true,
            dem_loaded: true,
            elevation: self
                .stitched_height(coords, *dem_coords, [wrapped_x, mercator_y], &dem.tile)
                .unwrap_or_else(|| dem.tile.elevation_at_tile_coords(x, y))
                * self.exaggeration,
        }
    }

    /// Exaggerated stitched elevation of the rendered tiles at Mercator coordinates, or `None`
    /// where no DEM has loaded.
    pub(crate) fn elevation_at(
        &self,
        tiles: &Tiles,
        mercator_x: f64,
        mercator_y: f64,
    ) -> Option<f64> {
        let sample = self.sample(tiles, mercator_x, mercator_y);
        sample.dem_loaded.then_some(sample.elevation)
    }

    /// Exaggerated elevation at Mercator coordinates in `0..1` from the best loaded DEM, rendered
    /// or not, bilinear within one DEM tile; `None` where nothing covering it has loaded. This
    /// keeps the camera above the ground, including ground the frame does not draw.
    pub fn loaded_ground_cached(
        &self,
        tiles: &Tiles,
        mercator_x: f64,
        mercator_y: f64,
        detail: DemDetail,
    ) -> Option<f64> {
        let zoom = match detail {
            DemDetail::Tile(zoom) => zoom,
            DemDetail::Finest => self.maxzoom.saturating_add(1).min(24),
        };
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

/// Exact view + DEM-bounds key for single-frame reuse of a tight [`crate::coords::ViewRegion`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct PreparedTerrainKey {
    dem_tile_size: u32,
    width_bits: u64,
    height_bits: u64,
    overscan_bits: u64,
    body_radius_bits: u64,
    external_view: bool,
    vp_bits: [[u64; 4]; 4],
    min_elev_bits: u64,
    max_elev_bits: u64,
    dem_bounds_len: usize,
}

impl PreparedTerrainKey {
    pub(crate) fn capture(
        view_state: &crate::render::view_state::ViewState,
        world: &crate::tcs::world::World,
        dem_tile_size: u32,
    ) -> Self {
        let m: [[f64; 4]; 4] = view_state.view_projection().0.into();
        let vp_bits = m.map(|col| col.map(f64::to_bits));
        let (min_elev_bits, max_elev_bits, dem_bounds_len) = world
            .resources
            .get::<TerrainCoverageIndex>()
            .map_or((0, 0, 0), |idx| {
                (
                    idx.min_elevation.to_bits(),
                    idx.max_elevation.to_bits(),
                    idx.dem_bounds.len(),
                )
            });
        Self {
            dem_tile_size,
            width_bits: view_state.width().to_bits(),
            height_bits: view_state.height().to_bits(),
            overscan_bits: view_state.request_overscan().to_bits(),
            body_radius_bits: view_state.body().radius_meters.to_bits(),
            external_view: view_state.has_external_view(),
            vp_bits,
            min_elev_bits,
            max_elev_bits,
            dem_bounds_len,
        }
    }
}

/// Cached tight [`crate::coords::ViewRegion`] from `coverage_system` or `multi_view` covering so
/// `Queue` (`target_specs` / `procedural_specs`) skips a duplicate S2 / frustum covering pass
/// when neither the camera projection nor DEM elevation bounds changed.
#[derive(Clone)]
pub(crate) struct PreparedTerrainRegion {
    pub(crate) key: PreparedTerrainKey,
    pub(crate) region: Option<crate::coords::ViewRegion>,
}

/// Takes the cached tight terrain region if its [`PreparedTerrainKey`] matches `view_state`
/// and `world`, otherwise computes a fresh `terrain_region`.
pub(crate) fn take_or_compute_tight_terrain_region(
    style: &crate::style::Style,
    view_state: &crate::render::view_state::ViewState,
    world: &mut crate::tcs::world::World,
    dem_tile_size: u32,
) -> Result<Option<crate::coords::ViewRegion>, crate::render::projection::ProjectionStateError> {
    if let Some(cached) = world.resources.remove::<PreparedTerrainRegion>() {
        if cached.key == PreparedTerrainKey::capture(view_state, world, dem_tile_size) {
            return Ok(cached.region);
        }
    }
    crate::render::projection::terrain_region(
        style,
        view_state,
        world,
        dem_tile_size,
        ViewStatePadding::Tight,
    )
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
    world.resources.remove::<PreparedTerrainRegion>();
    let Some(dem) = dem_source(style) else {
        world.resources.insert(TerrainCoverageIndex::default());
        return Ok(());
    };
    let prev_bounds = world
        .resources
        .get::<TerrainCoverageIndex>()
        .map(|idx| (idx.min_elevation, idx.max_elevation, idx.dem_bounds.len()));
    let rendered = crate::render::projection::terrain_region(
        style,
        view_state,
        world,
        dem.tile_size,
        ViewStatePadding::Tight,
    )
    .map_err(|error| {
        tracing::error!(%error, "unable to select terrain coverage tiles");
        crate::tcs::system::SystemError::Setup
    })?;
    let poles = world
        .resources
        .get::<super::PolarPoleElevations>()
        .copied()
        .unwrap_or_default();
    let index = match &rendered {
        Some(region) => TerrainCoverageIndex::build(
            region
                .iter()
                .filter(|coords| coords.build_quad_key().is_some()),
            &world.tiles,
            &dem,
        )
        .with_poles(poles),
        None => TerrainCoverageIndex::default().with_poles(poles),
    };
    let bounds_unchanged = prev_bounds.is_some_and(|(min_e, max_e, count)| {
        count == index.dem_bounds.len()
            && (min_e - index.min_elevation).abs() < 1e-3
            && (max_e - index.max_elevation).abs() < 1e-3
    });
    world.resources.insert(index);
    if bounds_unchanged {
        let key = PreparedTerrainKey::capture(view_state, world, dem.tile_size);
        world.resources.insert(PreparedTerrainRegion {
            key,
            region: rendered,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests;

mod surface;
