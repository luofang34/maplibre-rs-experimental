//! Single-frame cache for tight terrain [`crate::coords::ViewRegion`] coverings.

use super::TerrainCoverageIndex;
use crate::render::view_state::ViewStatePadding;

/// Exact view + DEM-bounds key for single-frame reuse of a tight [`crate::coords::ViewRegion`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct PreparedTerrainKey {
    dem_tile_size: u32,
    width_bits: u64,
    height_bits: u64,
    overscan_bits: u64,
    body_radius_bits: u64,
    center_elev_bits: u64,
    min_elev_bits: u64,
    globe_orbits_center: bool,
    pose_bits: Option<[u64; 9]>,
    external_view: bool,
    vp_bits: [[u64; 4]; 4],
    min_elev_idx_bits: u64,
    max_elev_idx_bits: u64,
    dem_bounds_len: usize,
}

impl PreparedTerrainKey {
    pub(crate) fn dem_tile_size(&self) -> u32 {
        self.dem_tile_size
    }

    pub(crate) fn capture(
        view_state: &crate::render::view_state::ViewState,
        world: &crate::tcs::world::World,
        dem_tile_size: u32,
    ) -> Self {
        let m: [[f64; 4]; 4] = view_state.view_projection().0.into();
        let vp_bits = m.map(|col| col.map(f64::to_bits));
        let (min_elev_idx_bits, max_elev_idx_bits, dem_bounds_len) = world
            .resources
            .get::<TerrainCoverageIndex>()
            .map_or((0, 0, 0), |idx| {
                (
                    idx.min_elevation.to_bits(),
                    idx.max_elevation.to_bits(),
                    idx.dem_bounds.len(),
                )
            });
        let pose_bits = view_state.globe_pose().map(|pose| {
            [
                pose.target[0].to_bits(),
                pose.target[1].to_bits(),
                pose.target[2].to_bits(),
                pose.target_elevation_meters.to_bits(),
                pose.orientation[0].to_bits(),
                pose.orientation[1].to_bits(),
                pose.orientation[2].to_bits(),
                pose.orientation[3].to_bits(),
                pose.distance_meters.to_bits(),
            ]
        });
        Self {
            dem_tile_size,
            width_bits: view_state.width().to_bits(),
            height_bits: view_state.height().to_bits(),
            overscan_bits: view_state.request_overscan().to_bits(),
            body_radius_bits: view_state.body().radius_meters.to_bits(),
            center_elev_bits: view_state.center_elevation().to_bits(),
            min_elev_bits: view_state.min_elevation().to_bits(),
            globe_orbits_center: view_state.globe_orbits_center(),
            pose_bits,
            external_view: view_state.has_external_view(),
            vp_bits,
            min_elev_idx_bits,
            max_elev_idx_bits,
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
