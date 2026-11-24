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
    tcs::{pyramid::TilePyramid, system::SystemResult, tiles::Tiles, world::World},
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
        .is_none_or(|idx| idx.dem_bounds_sig != dem_sig || idx.dem_bounds.len() != dem_count);
    let bounds = needs_rebuild.then(|| build_dem_bounds(&world.tiles));
    let index = world.resources.get_or_init_mut::<TerrainCoverageIndex>();
    if let Some(bounds) = bounds {
        index.sync_dem_bounds_in_place(bounds, dem_sig, &dem, poles);
    } else if index.poles != poles
        || index.exaggeration.to_bits() != key.exaggeration_bits
        || index.minzoom != dem.minzoom
        || index.maxzoom != dem.maxzoom
        || index.dem_tile_size != dem.tile_size
    {
        let reused = std::mem::take(&mut index.dem_bounds);
        index.sync_dem_bounds_in_place(reused, dem_sig, &dem, poles);
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
            loaded.tile.min().to_bits().hash(&mut h);
            loaded.tile.max().to_bits().hash(&mut h);
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
            bounds.insert(coords, (dem.tile.min(), dem.tile.max()));
        }
    }
    bounds
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
            crate::tcs::system::SystemError::Setup
        })?;
    let poles = world
        .resources
        .get::<PolarPoleElevations>()
        .copied()
        .unwrap_or_default();
    let sync_key = DemBoundsSyncKey::capture(world, &dem, poles);
    let (dem_sig, dem_count) = match (
        world.resources.get::<DemBoundsSyncState>(),
        world.resources.get::<TerrainCoverageIndex>(),
    ) {
        (Some(sync), Some(idx)) if sync.0 == sync_key => (idx.dem_bounds_sig, idx.dem_bounds.len()),
        _ => dem_bounds_signature(&world.tiles),
    };
    if world
        .resources
        .get::<TerrainCoverageIndex>()
        .is_some_and(|prev| {
            !prev.rendered_stale
                && prev.matches_inputs(rendered.as_ref(), (dem_sig, dem_count), &dem, poles)
        })
    {
        let key = PreparedTerrainKey::capture_for_style(style, view_state, world, dem.tile_size);
        world.resources.insert(PreparedTerrainRegion {
            key,
            region: rendered,
        });
        return Ok(());
    }
    let mut prev_bounds = None;
    let reused_dem_bounds =
        world
            .resources
            .get_mut::<TerrainCoverageIndex>()
            .and_then(|prev_idx| {
                prev_bounds = Some((
                    prev_idx.min_elevation,
                    prev_idx.max_elevation,
                    prev_idx.dem_bounds.len(),
                ));
                (prev_idx.dem_bounds_sig == dem_sig && prev_idx.dem_bounds.len() == dem_count)
                    .then(|| std::mem::take(&mut prev_idx.dem_bounds))
            });
    let bounds = reused_dem_bounds.unwrap_or_else(|| build_dem_bounds(&world.tiles));
    let index = match &rendered {
        Some(region) => TerrainCoverageIndex::build_with_dem_bounds(
            region
                .iter()
                .filter(|coords| coords.build_quad_key().is_some()),
            bounds,
            dem_sig,
            &dem,
        )
        .with_poles(poles),
        None => TerrainCoverageIndex {
            dem_bounds: bounds,
            dem_bounds_sig: dem_sig,
            ..TerrainCoverageIndex::default()
        }
        .with_poles(poles),
    };
    let bounds_unchanged = prev_bounds.is_some_and(|(min_e, max_e, count)| {
        count == index.dem_bounds.len()
            && (min_e - index.min_elevation).abs() < 1e-3
            && (max_e - index.max_elevation).abs() < 1e-3
    });
    world.resources.insert(index);
    if bounds_unchanged {
        let key = PreparedTerrainKey::capture_for_style(style, view_state, world, dem.tile_size);
        world.resources.insert(PreparedTerrainRegion {
            key,
            region: rendered,
        });
    }
    Ok(())
}
