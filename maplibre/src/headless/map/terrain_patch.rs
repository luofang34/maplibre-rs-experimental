//! Zero-drape procedural relief controls and stitched resident DEM patch export.

use std::sync::Arc;

use super::HeadlessMap;
use crate::{
    coords::WorldTileCoords,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        settings::{ProceduralReliefStyle, TerrainDrapeMode},
    },
    terrain::{
        coverage::TerrainCoverageIndex, dem::DemTile, mesh::TERRAIN_MESH_SIZE,
        resources::TerrainResources, source::dem_source,
    },
};

/// Immutable CPU + GPU snapshot of a resident terrain surface patch (`129 × 129` vertices)
/// exported for external volumetric / XVS / custom 3D consumers.
#[derive(Clone, Debug)]
pub struct TerrainPatchSnapshot {
    /// Coordinates of the queried terrain surface patch.
    pub coords: WorldTileCoords,
    /// Coordinates of the loaded DEM tile backing `coords` (`dem_coords.z <= coords.z`).
    pub dem_coords: WorldTileCoords,
    /// Shared immutable decoded DEM raster grid.
    pub dem_tile: Arc<DemTile>,
    /// Number of vertices along each edge of `stitched_heights_meters` (`129`).
    pub grid_side: u32,
    /// Row-major `grid_side × grid_side` exaggerated elevations in metres MSL, stitched along
    /// tile borders to match neighbor LODs without T-junction cracks.
    pub stitched_heights_meters: Arc<[f32]>,
    /// Minimum vertex elevation in `stitched_heights_meters` (metres MSL, exaggerated).
    pub min_elevation_meters: f32,
    /// Maximum vertex elevation in `stitched_heights_meters` (metres MSL, exaggerated).
    pub max_elevation_meters: f32,
    /// Vertical exaggeration factor applied to `stitched_heights_meters`.
    pub exaggeration: f32,
    /// `true` when rendered neighbor `EdgeHeights` were applied along the patch boundary.
    pub stitched_to_neighbors: bool,
    /// Resident mipmapped drape texture covering `coords` (its own or nearest resident ancestor)
    /// when [`TerrainDrapeMode::Draped`] is active; `None` in [`TerrainDrapeMode::ProceduralRelief`].
    pub drape_texture: Option<wgpu::Texture>,
}

impl TerrainPatchSnapshot {
    /// Returns the stitched elevation (metres MSL) at `(col, row)` in `0..grid_side`.
    pub fn height_at_grid(&self, col: u32, row: u32) -> Option<f32> {
        if col >= self.grid_side || row >= self.grid_side {
            return None;
        }
        let idx = (row as usize) * (self.grid_side as usize) + (col as usize);
        self.stitched_heights_meters.get(idx).copied()
    }
}

impl HeadlessMap {
    /// Returns the active [`TerrainDrapeMode`] of this map.
    pub fn terrain_drape_mode(&self) -> TerrainDrapeMode {
        self.map_context
            .world
            .resources
            .get::<TerrainDrapeMode>()
            .copied()
            .unwrap_or_else(|| {
                self.map_context
                    .renderer
                    .settings
                    .effective_terrain_drape_mode()
            })
    }

    /// Switches between baked 2D style drapes ([`TerrainDrapeMode::Draped`]) and zero-drape
    /// procedural DEM shading ([`TerrainDrapeMode::ProceduralRelief`]).
    ///
    /// Switching to `ProceduralRelief` immediately releases any allocated `DrapeScratch` and
    /// `DrapeCache` textures.
    pub fn set_terrain_drape_mode(&mut self, mode: TerrainDrapeMode) {
        self.map_context.renderer.settings.terrain_drape_mode = mode;
        if let TerrainDrapeMode::Draped { edge_px } = mode {
            self.map_context.renderer.settings.terrain_drape_size = edge_px;
        }
        self.map_context.world.resources.insert(mode);
        if let Some(Initialized(terrain)) = self
            .map_context
            .world
            .resources
            .get_mut::<Eventually<TerrainResources>>()
        {
            terrain.set_drape_mode(&self.map_context.renderer.device, mode);
        }
        crate::render::frame_signals::mark_dirty(&mut self.map_context.world);
    }

    /// Updates the TAWS reference altitude (metres MSL) in [`TerrainDrapeMode::ProceduralRelief`]
    /// without reallocating GPU resources. Has no effect when in [`TerrainDrapeMode::Draped`].
    pub fn set_procedural_relief_reference_altitude(&mut self, altitude_meters: Option<f32>) {
        if let TerrainDrapeMode::ProceduralRelief(mut style) = self.terrain_drape_mode() {
            style.reference_altitude_meters = altitude_meters;
            self.set_terrain_drape_mode(TerrainDrapeMode::ProceduralRelief(style));
        } else {
            let _ = ProceduralReliefStyle::default();
        }
    }

    /// Samples `(bilinear_msl_meters, cell_max_msl_meters)` in unexaggerated orthometric metres
    /// at normalized `[0, 1]` tile coordinates `(u, v)` on `coords`'s `128 × 128` terrain mesh
    /// without allocating a full `129 × 129` [`TerrainPatchSnapshot`].
    pub fn sample_resident_dem_grid_cell(
        &self,
        coords: WorldTileCoords,
        u: f64,
        v: f64,
    ) -> Option<(f64, f64)> {
        let context = &self.map_context;
        let fallback_dem =
            dem_source(&context.style).map(|dem| (dem.minzoom, dem.maxzoom, dem.exaggeration));
        let empty_index = TerrainCoverageIndex::default();
        let index = context
            .world
            .resources
            .get::<TerrainCoverageIndex>()
            .unwrap_or(&empty_index);
        index.sample_stitched_patch_cell_msl(&context.world.tiles, coords, (u, v), fallback_dem)
    }

    /// Exports a stitched `129 × 129` (`TERRAIN_MESH_SIZE + 1`) elevation grid and optional
    /// resident drape texture for `coords` if a covering DEM tile is resident in `World`.
    pub fn resident_terrain_patch(&self, coords: WorldTileCoords) -> Option<TerrainPatchSnapshot> {
        let context = &self.map_context;
        let fallback_dem =
            dem_source(&context.style).map(|dem| (dem.minzoom, dem.maxzoom, dem.exaggeration));
        let empty_index = TerrainCoverageIndex::default();
        let index = context
            .world
            .resources
            .get::<TerrainCoverageIndex>()
            .unwrap_or(&empty_index);
        let grid = index.stitched_patch_grid(&context.world.tiles, coords, fallback_dem)?;

        let drape_texture = if self.terrain_drape_mode().is_procedural_relief() {
            None
        } else {
            match context
                .world
                .resources
                .get::<Eventually<TerrainResources>>()
            {
                Some(Initialized(terrain)) => {
                    let mut cur = Some(coords);
                    let mut found = None;
                    while let Some(c) = cur {
                        if let Some(tex) = terrain.drape_texture(c) {
                            found = Some(tex.texture.clone());
                            break;
                        }
                        cur = c.get_parent();
                    }
                    found
                }
                _ => None,
            }
        };

        Some(TerrainPatchSnapshot {
            coords,
            dem_coords: grid.dem_coords,
            dem_tile: grid.dem_tile,
            grid_side: TERRAIN_MESH_SIZE + 1,
            stitched_heights_meters: grid.heights_meters,
            min_elevation_meters: grid.min_meters,
            max_elevation_meters: grid.max_meters,
            exaggeration: grid.exaggeration,
            stitched_to_neighbors: grid.stitched_to_neighbors,
            drape_texture,
        })
    }
}

#[cfg(test)]
mod tests;
