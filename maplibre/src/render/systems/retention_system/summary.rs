//! Periodic resident-memory, GPU-object, and frame-timing diagnostics for external-eye hosts.

use super::{TileRetention, SUMMARY_EVERY_FRAMES};
use crate::{
    render::{
        eventually::{Eventually, Eventually::Initialized},
        tile_memory::tile_bytes,
    },
    tcs::{
        system::{heap::live_bytes, timings::FrameTimings},
        world::World,
    },
    terrain::resources::TerrainResources,
    vector::VectorBufferPool,
};

/// Once a second, how many GPU objects the frame keeps alive, for a host whose footprint
/// climbs while nothing loads: the kind whose count climbs with it is the one leaking.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn summarize_gpu_objects(world: &World, renderer: &crate::render::Renderer) {
    let frame = world
        .resources
        .get::<TileRetention>()
        .map_or(0, |retention| retention.frame);
    if !frame.is_multiple_of(SUMMARY_EVERY_FRAMES) {
        return;
    }
    let Some(report) = renderer.instance.generate_report() else {
        return;
    };
    let hub = report.hub_report();
    tracing::info!(
        buffers = hub.buffers.num_allocated,
        textures = hub.textures.num_allocated,
        textures_live = hub.textures.num_kept_from_user,
        texture_views = hub.texture_views.num_allocated,
        views_live = hub.texture_views.num_kept_from_user,
        bind_groups = hub.bind_groups.num_allocated,
        samplers = hub.samplers.num_allocated,
        command_buffers = hub.command_buffers.num_allocated,
        pipelines = hub.render_pipelines.num_allocated,
        "gpu objects"
    );
}

/// Once a second, the costliest systems and stages of the frame, for a host that cannot
/// attach a profiler.
pub(super) fn summarize_timings(world: &mut World) {
    let frame = world
        .resources
        .get::<TileRetention>()
        .map_or(0, |retention| retention.frame);
    if !frame.is_multiple_of(SUMMARY_EVERY_FRAMES) {
        return;
    }
    let timings = world.resources.get_or_init_mut::<FrameTimings>();
    let growers: Vec<String> = timings
        .top_growth(6)
        .iter()
        .filter(|(_, kb)| *kb > 0.5)
        .map(|(name, kb)| format!("{name}={kb:.1}"))
        .collect();
    let top = timings.take_top(8);
    let costliest: Vec<String> = top
        .iter()
        .map(|(name, ms)| format!("{name}={ms:.2}"))
        .collect();
    tracing::info!(ms_per_frame = %costliest.join(" "), "frame time by system");
    if !growers.is_empty() {
        tracing::info!(
            kb_per_frame = %growers.join(" "),
            rust_heap_mb = live_bytes(&world.resources) >> 20,
            "heap growth by system"
        );
    }
}

pub(super) fn summarize_residency(world: &World, drawn: usize, in_use: usize, settled: bool) {
    let frame = world
        .resources
        .get::<TileRetention>()
        .map_or(0, |retention| retention.frame);
    if !frame.is_multiple_of(SUMMARY_EVERY_FRAMES) {
        return;
    }
    let tiles = world.tiles.tiles.len();
    let bytes: usize = world
        .tiles
        .tiles
        .values()
        .map(|tile| tile_bytes(&world.tiles, tile.coords))
        .sum();
    let index_bytes = world.tiles.geometry_index.approximate_bytes();
    let (pool_revision, vec_pool_bytes) =
        match world.resources.get::<Eventually<VectorBufferPool>>() {
            Some(Initialized(pool)) => (pool.revision(), pool.device_buffer_bytes()),
            _ => (0, 0),
        };
    let (symbol_revision, sym_pool_bytes) = match world
        .resources
        .get::<Eventually<crate::sdf::SymbolBufferPool>>()
    {
        Some(Initialized(pool)) => (pool.revision(), pool.device_buffer_bytes()),
        _ => (0, 0),
    };
    let (pool_dem_textures, pool_dem_bytes) = world
        .resources
        .get::<crate::terrain::GpuDemPool>()
        .map_or((0, 0), |pool| (pool.texture_count(), pool.texture_bytes()));
    let (drapes, free_drapes, drape_bytes, dem_textures, dem_bytes) =
        match world.resources.get::<Eventually<TerrainResources>>() {
            Some(Initialized(terrain)) => {
                let (held, free) = terrain.drape_counts();
                let (drape_bytes, dem_bytes) = terrain.texture_bytes();
                (
                    held,
                    free,
                    drape_bytes,
                    terrain.dem_texture_count().max(pool_dem_textures),
                    dem_bytes.max(pool_dem_bytes),
                )
            }
            _ => (0, 0, 0, pool_dem_textures, pool_dem_bytes),
        };
    tracing::info!(
        tiles,
        pending = crate::io::tile_backpressure::tiles_in_flight(world),
        settled,
        geometry_mb = bytes >> 20,
        gpu_pool_mb = (vec_pool_bytes + sym_pool_bytes) >> 20,
        index_mb = index_bytes >> 20,
        pool_revision,
        drawn,
        in_use,
        drapes,
        free_drapes,
        drape_mb = drape_bytes >> 20,
        dem_textures,
        dem_mb = dem_bytes >> 20,
        finest_dem_zoom = finest_dem_zoom(world),
        symbol_revision,
        "resident tiles"
    );
}

fn finest_dem_zoom(world: &World) -> u8 {
    world
        .tiles
        .tiles
        .values()
        .filter_map(|tile| {
            matches!(
                world
                    .tiles
                    .query::<&crate::terrain::DemTileComponent>(tile.coords),
                Some(crate::terrain::DemTileComponent::Loaded(_))
            )
            .then_some(u8::from(tile.coords.z))
        })
        .max()
        .unwrap_or(0)
}
