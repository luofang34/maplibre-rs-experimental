//! Loaded-DEM pyramid caching and per-frame coverage index update system.

use std::hash::{Hash, Hasher};

use rustc_hash::FxHasher;

use super::{
    prepared::{take_or_compute_tight_terrain_region, PreparedTerrainKey, PreparedTerrainRegion},
    PolarPoleElevations, TerrainCoverageIndex,
};
use crate::{
    context::MapContext,
    render::eye_covering::EyeInFrame,
    schedule::system::SystemResult,
    tcs::{pyramid::TilePyramid, tiles::Tiles, world::World},
    terrain::{source::dem_source, DemTileComponent},
};

#[derive(Clone, Copy, PartialEq, Eq)]
struct DemBoundsSyncKey {
    arrival_epoch: u64,
    pyramid_len: usize,
    exaggeration_bits: u64,
    minzoom: u8,
    maxzoom: u8,
    dem_tile_size: u32,
    poles_bits: [u32; 2],
}

impl DemBoundsSyncKey {
    fn capture(
        world: &World,
        dem: &crate::terrain::source::DemSource,
        poles: PolarPoleElevations,
    ) -> Self {
        Self {
            arrival_epoch: crate::terrain::dem_arrival_epoch(world),
            pyramid_len: world.tiles.pyramid_of::<DemTileComponent>().len(),
            exaggeration_bits: f64::from(dem.exaggeration).to_bits(),
            minzoom: dem.minzoom,
            maxzoom: dem.maxzoom,
            dem_tile_size: dem.tile_size,
            poles_bits: [poles.north_meters.to_bits(), poles.south_meters.to_bits()],
        }
    }
}

#[derive(Clone, Copy)]
struct DemBoundsSyncState(DemBoundsSyncKey);

fn clear_missing_dem_state(world: &mut World) {
    if world.resources.get::<PreparedTerrainRegion>().is_some() {
        world.resources.remove::<PreparedTerrainRegion>();
    }
    if world.resources.get::<DemBoundsSyncState>().is_some() {
        world.resources.remove::<DemBoundsSyncState>();
    }
    if world
        .resources
        .get::<TerrainCoverageIndex>()
        .is_some_and(|idx| !idx.is_empty())
    {
        world.resources.insert(TerrainCoverageIndex::default());
        super::install_elevation_bridge(world);
    }
}

impl TerrainCoverageIndex {
    fn matches_dem_bounds(&self, dem_sig: u64, dem_count: usize) -> bool {
        self.dem_bounds_sig == dem_sig && self.dem_bounds.len() == dem_count
    }

    fn needs_param_resync(
        &self,
        dem: &crate::terrain::source::DemSource,
        poles: PolarPoleElevations,
    ) -> bool {
        self.poles != poles
            || self.exaggeration.to_bits() != f64::from(dem.exaggeration).to_bits()
            || self.minzoom != dem.minzoom
            || self.maxzoom != dem.maxzoom
            || self.dem_tile_size != dem.tile_size
    }

    fn resync_params_in_place(
        &mut self,
        dem_sig: u64,
        dem: &crate::terrain::source::DemSource,
        poles: PolarPoleElevations,
    ) {
        let reused = std::mem::take(&mut self.dem_bounds);
        self.sync_dem_bounds_in_place(reused, dem_sig, dem, poles);
    }

    fn dem_bounds_signature_and_len(&self) -> (u64, usize) {
        (self.dem_bounds_sig, self.dem_bounds.len())
    }

    fn take_reusable_dem_bounds(
        &mut self,
        dem_sig: u64,
        dem_count: usize,
    ) -> Option<TilePyramid<(f64, f64)>> {
        self.matches_dem_bounds(dem_sig, dem_count)
            .then(|| std::mem::take(&mut self.dem_bounds))
    }

    fn with_empty_rendered(
        dem_bounds: TilePyramid<(f64, f64)>,
        dem_bounds_sig: u64,
        poles: PolarPoleElevations,
    ) -> Self {
        Self {
            dem_bounds,
            dem_bounds_sig,
            ..Self::default()
        }
        .with_poles(poles)
    }
}

/// Syncs loaded DEM tile bounds into [`TerrainCoverageIndex`] before center elevation runs.
pub fn sync_dem_bounds_system(MapContext { style, world, .. }: &mut MapContext) -> SystemResult {
    if EyeInFrame::reuses_content(world) {
        return Ok(());
    }
    let Some(dem) = dem_source(style) else {
        clear_missing_dem_state(world);
        return Ok(());
    };
    let poles = world
        .resources
        .get::<PolarPoleElevations>()
        .copied()
        .unwrap_or_default();
    let key = DemBoundsSyncKey::capture(world, &dem, poles);
    let already_synced = world
        .resources
        .get::<DemBoundsSyncState>()
        .is_some_and(|s| s.0 == key)
        && world.resources.get::<TerrainCoverageIndex>().is_some();
    if already_synced {
        return Ok(());
    }
    let (dem_sig, dem_count) = dem_bounds_signature(&world.tiles);
    let needs_rebuild = world
        .resources
        .get::<TerrainCoverageIndex>()
        .is_none_or(|idx| !idx.matches_dem_bounds(dem_sig, dem_count));
    super::install_elevation_bridge(world);
    let bounds = needs_rebuild.then(|| build_dem_bounds(&world.tiles));
    let index = world.resources.get_or_init_mut::<TerrainCoverageIndex>();
    if let Some(bounds) = bounds {
        index.sync_dem_bounds_in_place(bounds, dem_sig, &dem, poles);
    } else if index.needs_param_resync(&dem, poles) {
        index.resync_params_in_place(dem_sig, &dem, poles);
    }
    world.resources.insert(DemBoundsSyncState(key));
    Ok(())
}

pub(super) fn dem_bounds_signature(tiles: &Tiles) -> (u64, usize) {
    let mut h = FxHasher::default();
    let mut count = 0_usize;
    for coords in tiles
        .pyramid_of::<DemTileComponent>()
        .coords_coarse_to_fine()
    {
        if let Some(DemTileComponent::Loaded(loaded)) = tiles.query::<&DemTileComponent>(coords) {
            count += 1;
            coords.hash(&mut h);
            loaded.min().to_bits().hash(&mut h);
            loaded.max().to_bits().hash(&mut h);
        }
    }
    count.hash(&mut h);
    (h.finish(), count)
}

pub(super) fn build_dem_bounds(tiles: &Tiles) -> TilePyramid<(f64, f64)> {
    let mut bounds = TilePyramid::new();
    for coords in tiles
        .pyramid_of::<DemTileComponent>()
        .coords_coarse_to_fine()
    {
        if let Some(DemTileComponent::Loaded(dem)) = tiles.query::<&DemTileComponent>(coords) {
            bounds.insert(coords, (dem.min(), dem.max()));
        }
    }
    bounds
}

fn resolve_dem_signature(world: &World, sync_key: DemBoundsSyncKey) -> (u64, usize) {
    match (
        world.resources.get::<DemBoundsSyncState>(),
        world.resources.get::<TerrainCoverageIndex>(),
    ) {
        (Some(sync), Some(idx)) if sync.0 == sync_key => idx.dem_bounds_signature_and_len(),
        _ => dem_bounds_signature(&world.tiles),
    }
}

fn cache_prepared_region(
    world: &mut World,
    style: &crate::style::Style,
    view_state: &crate::render::view_state::ViewState,
    tile_size: u32,
    region: Option<crate::coords::ViewRegion>,
) {
    let key = PreparedTerrainKey::capture_for_style(style, view_state, world, tile_size);
    world
        .resources
        .insert(PreparedTerrainRegion { key, region });
}

fn rebuild_coverage_index(
    world: &mut World,
    rendered: Option<&crate::coords::ViewRegion>,
    (dem_sig, dem_count): (u64, usize),
    dem: &crate::terrain::source::DemSource,
    poles: PolarPoleElevations,
) -> (TerrainCoverageIndex, Option<(f64, f64, usize)>) {
    let mut prev_bounds = None;
    let reused = world
        .resources
        .get_mut::<TerrainCoverageIndex>()
        .and_then(|prev_idx| {
            prev_bounds = Some((
                prev_idx.min_elevation,
                prev_idx.max_elevation,
                prev_idx.dem_bounds_len(),
            ));
            prev_idx.take_reusable_dem_bounds(dem_sig, dem_count)
        });
    let bounds = reused.unwrap_or_else(|| build_dem_bounds(&world.tiles));
    let index = match rendered {
        Some(region) => TerrainCoverageIndex::build_with_dem_bounds(
            region
                .iter()
                .filter(|coords| coords.build_quad_key().is_some()),
            bounds,
            dem_sig,
            dem,
        )
        .with_poles(poles),
        None => TerrainCoverageIndex::with_empty_rendered(bounds, dem_sig, poles),
    };
    (index, prev_bounds)
}

/// Rebuilds the coverage index from the tiles the terrain will draw this frame, reusing cached
/// DEM bounds when the loaded DEM pyramid is unchanged.
pub fn coverage_system(
    MapContext {
        style,
        view_state,
        world,
        ..
    }: &mut MapContext,
) -> SystemResult {
    if EyeInFrame::reuses_content(world) {
        return Ok(());
    }
    let Some(dem) = dem_source(style) else {
        clear_missing_dem_state(world);
        return Ok(());
    };
    let rendered = take_or_compute_tight_terrain_region(style, view_state, world, dem.tile_size)
        .map_err(|error| {
            tracing::error!(%error, "unable to select terrain coverage tiles");
            crate::schedule::system::SystemError::Setup
        })?;
    let poles = world
        .resources
        .get::<PolarPoleElevations>()
        .copied()
        .unwrap_or_default();
    let sync_key = DemBoundsSyncKey::capture(world, &dem, poles);
    let (dem_sig, dem_count) = resolve_dem_signature(world, sync_key);
    if world
        .resources
        .get::<TerrainCoverageIndex>()
        .is_some_and(|prev| {
            !prev.rendered_stale
                && prev.matches_inputs(rendered.as_ref(), (dem_sig, dem_count), &dem, poles)
        })
    {
        cache_prepared_region(world, style, view_state, dem.tile_size, rendered);
        return Ok(());
    }
    let (index, prev_bounds) =
        rebuild_coverage_index(world, rendered.as_ref(), (dem_sig, dem_count), &dem, poles);
    let bounds_unchanged = prev_bounds.is_some_and(|(min_e, max_e, count)| {
        count == index.dem_bounds_len()
            && (min_e - index.min_elevation).abs() < 1e-3
            && (max_e - index.max_elevation).abs() < 1e-3
    });
    world.resources.insert(index);
    super::install_elevation_bridge(world);
    if bounds_unchanged {
        cache_prepared_region(world, style, view_state, dem.tile_size, rendered);
    }
    Ok(())
}

fn get_elevation_source(
    world: &World,
) -> Option<&dyn crate::render::projection::TerrainElevationSource> {
    world
        .resources
        .get::<TerrainCoverageIndex>()
        .map(|idx| idx as &dyn crate::render::projection::TerrainElevationSource)
}

/// Registers terrain's coverage index and drawn-surface callbacks for projection covering.
pub(crate) fn install_elevation_bridge(world: &mut World) {
    if world
        .resources
        .get::<crate::render::projection::TerrainElevationBridge>()
        .is_none()
    {
        world
            .resources
            .insert(crate::render::projection::TerrainElevationBridge {
                get_index: get_elevation_source,
                drawn_surface_tiles: super::drawn_surface_tiles,
            });
    }
}
