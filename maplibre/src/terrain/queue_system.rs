//! Builds the drape targets and terrain draws for the current frame.

use crate::{
    context::MapContext,
    coords::WorldTileCoords,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        eye_covering::EyeInFrame,
        resolved_view::ResolvedFrameView,
        tile_view_pattern::DEFAULT_TILE_SIZE,
        view_state::ViewState,
        Renderer,
    },
    style::Style,
    tcs::{
        system::{SystemError, SystemResult},
        world::World,
    },
    terrain::{
        resources::TerrainResources,
        source::{dem_source, DemSource},
        DrapePhase, TerrainFrame,
    },
};
mod acquire;
mod activity;
pub(crate) use activity::PendingDrapes;
mod cohort;
mod covering;
mod drape_prepare;
pub(crate) mod edges;
mod prefetch;
mod readiness;
mod spec_cache;
mod steady_cache;
mod targets;
#[cfg(any(test, feature = "headless"))]
pub(crate) use targets::target_specs;
pub(crate) use targets::{
    draw_order_key, draw_order_key_with_resolved, procedural_tiles, target_specs_arc,
};
mod drape_phase;
mod frame;
pub(crate) mod surface_covering;
pub(crate) mod uniforms;
use drape_phase::encode_drapes;
#[cfg(all(test, feature = "headless"))]
use drape_prepare::loaded_content;
use drape_prepare::{prepare_drapes, PreparedDrapes};
use frame::{hide_draped_layers, set_frame};
pub(crate) use uniforms::TerrainEyeFrame;

/// Divisor of the tile circumference giving the skirt drop, as in GL JS `getSkirtLength`.
const SKIRT_DIVISOR: f64 = 5.0;
/// Drape textures drawn in one frame. Every level of a drape's mip chain is a render pass,
/// and WebGPU / Metal close a pass per target + mip level; keeping `wasm32` tightly bounded
/// avoids 300–500 ms browser render-pass storms while remaining tiles use ancestors or wait 1–2 frames.
#[cfg(target_arch = "wasm32")]
const MAX_DRAPES_PER_FRAME: usize = 5;
#[cfg(not(target_arch = "wasm32"))]
const MAX_DRAPES_PER_FRAME: usize = 24;
/// Drape textures drawn per frame while a host's eye drives the map. A flight lands hundreds
/// of tiles within a few frames, and a display's frame budget holds only a few drapes.
#[cfg(target_arch = "wasm32")]
const EYE_DRAPES_PER_FRAME: usize = 3;
#[cfg(not(target_arch = "wasm32"))]
const EYE_DRAPES_PER_FRAME: usize = 8;

/// How many drapes a frame draws, the rest waiting for later frames.
#[derive(Clone, Copy, Debug)]
pub struct DrapeBudget {
    /// Drapes per frame of the map's own camera.
    pub per_frame: usize,
    /// Drapes per frame while a host's eye drives the map.
    pub per_eye_frame: usize,
    /// GPU time a frame spends on drapes while the camera moves; the drapes past it keep their
    /// previous or an ancestor's texture until a later frame.
    pub moving_time: std::time::Duration,
}

impl Default for DrapeBudget {
    fn default() -> Self {
        #[cfg(target_arch = "wasm32")]
        let moving_time = std::time::Duration::from_millis(2);
        #[cfg(not(target_arch = "wasm32"))]
        let moving_time = std::time::Duration::from_millis(4);
        Self {
            per_frame: MAX_DRAPES_PER_FRAME,
            per_eye_frame: EYE_DRAPES_PER_FRAME,
            moving_time,
        }
    }
}

#[derive(Default)]
struct DrapeFingerprintZoom(Option<f64>);

/// Quantizes drape zoom invalidation to integer zoom crossings with `0.15`-level hysteresis so
/// fractional `0.125` `paint_zoom` steps or DEM camera-reconciliation nudges do not invalidate
/// every cached `1024²` drape texture simultaneously.
pub(crate) fn next_drape_fingerprint_zoom(previous: Option<f64>, requested: f64) -> f64 {
    previous
        .filter(|&prev| requested >= prev - 0.15 && requested <= prev + 1.15)
        .unwrap_or_else(|| requested.floor())
}

fn stabilize_drape_fingerprint_zoom(world: &mut World, requested: f64) -> f64 {
    let state = world.resources.get_or_init_mut::<DrapeFingerprintZoom>();
    let zoom = next_drape_fingerprint_zoom(state.0, requested);
    state.0 = Some(zoom);
    zoom
}

pub fn queue_system(
    MapContext {
        style,
        view_state,
        world,
        renderer:
            Renderer {
                device,
                queue,
                settings,
                ..
            },
        ..
    }: &mut MapContext,
) -> SystemResult {
    if world
        .resources
        .get::<crate::render::settings::TightTileRequests>()
        .is_none_or(|tight| tight.0 != settings.tight_tile_requests)
    {
        world
            .resources
            .insert(crate::render::settings::TightTileRequests(
                settings.tight_tile_requests,
            ));
    }
    let Some(dem) = dem_source(style) else {
        if let Some(terrain) = world.resources.get_mut::<Eventually<TerrainResources>>() {
            if matches!(terrain, Initialized(_)) {
                *terrain = Eventually::Uninitialized;
            }
        }
        if let Some(dem_pool) = world.resources.get_mut::<super::gpu_dem_pool::GpuDemPool>()
            && dem_pool.texture_count() > 0
        {
            dem_pool.retain_sources(|id| {
                id.name().is_some_and(|name| {
                    matches!(
                        style.sources.get(name),
                        Some(crate::style::source::Source::RasterDem(_))
                    )
                })
            });
        }
        let was_active = world
            .resources
            .get::<TerrainFrame>()
            .is_some_and(|frame| frame.active);
        if was_active
            || world
                .resources
                .get::<DrapePhase>()
                .is_some_and(|phase| !phase.targets.is_empty())
        {
            world.resources.insert(TerrainEyeFrame::default());
            world
                .resources
                .insert(surface_covering::SurfaceTiles::default());
            world.resources.insert(DrapePhase::default());
            set_frame(world, TerrainFrame::default());
        } else if world
            .resources
            .get::<crate::render::ground::FrameGround>()
            .is_none_or(|g| g.0.is_some())
        {
            set_frame(world, TerrainFrame::default());
        }
        return Ok(());
    };
    let drape_mode = world
        .resources
        .get::<crate::render::settings::TerrainDrapeMode>()
        .copied()
        .unwrap_or_else(|| settings.effective_terrain_drape_mode());
    let drape_view = world
        .resources
        .get::<crate::render::eye_covering::DrapeView>()
        .and_then(|view| view.0);
    if let Some(Initialized(terrain)) = world.resources.get_mut::<Eventually<TerrainResources>>() {
        terrain.show_drapes_of(drape_view);
        if terrain.drape_mode() != drape_mode {
            terrain.set_drape_mode(device, drape_mode);
        }
    }
    if EyeInFrame::reuses_content(world) {
        return uniforms::replay(world, style, view_state, queue);
    }
    if steady_cache::needs_eye_frame(world)
        || world
            .resources
            .get::<TerrainEyeFrame>()
            .is_some_and(|frame| !frame.0.is_empty())
    {
        world.resources.insert(TerrainEyeFrame::default());
    }
    world
        .resources
        .insert(surface_covering::SurfaceTiles::default());
    let (phase, surface_coords, surface_sources) = if drape_mode.is_procedural_relief() {
        let coords = procedural_tiles(style, view_state, world)?;
        let sources = vec![None; coords.len()];
        (DrapePhase::default(), coords, sources)
    } else {
        let specs = target_specs_arc(style, view_state, world)?;
        let drape_zoom = stabilize_drape_fingerprint_zoom(world, view_state.style_zoom().value());
        let PreparedDrapes {
            redraw,
            sources,
            clear_color,
        } = prepare_drapes(&specs, style, view_state, world, device, drape_zoom)?;
        let encode_started = instant::Instant::now();
        let phase = encode_drapes(
            &specs,
            &redraw,
            clear_color,
            world,
            crate::coords::Zoom::new(drape_zoom),
            queue,
        )?;
        if !phase.targets.is_empty() {
            if let Some(slot) = world
                .resources
                .get::<crate::terrain::drape_timing::DrapeTimerSlot>()
            {
                slot.record_encode_cpu(encode_started.elapsed());
            }
        }
        let (surface_coords, surface_sources) =
            surface_covering::coords_for_frame(world, &specs, &sources);
        (phase, surface_coords, surface_sources)
    };
    queue_tiles(
        world,
        style,
        view_state,
        device,
        queue,
        &dem,
        (&surface_coords, &surface_sources),
    )?;
    hide_draped_layers(world, style);
    let draw_after_layer_index = style
        .layers
        .iter()
        .filter(|layer| layer.type_ == "background")
        .map(|layer| layer.index)
        .max()
        .unwrap_or(0);
    world.resources.insert(phase);
    set_frame(
        world,
        TerrainFrame {
            active: true,
            draw_after_layer_index,
        },
    );
    Ok(())
}

#[cfg(test)]
mod tests;

fn queue_tiles(
    world: &mut World,
    style: &Style,
    view_state: &ViewState,
    device: &wgpu::Device,
    queue: &crate::render::upload_queue::UploadQueue,
    dem: &DemSource,
    targets: (&[WorldTileCoords], &[Option<WorldTileCoords>]),
) -> SystemResult {
    let mut edge_cache = std::mem::take(world.resources.get_or_init_mut::<edges::EdgeCache>());
    let mut queue_scratch = std::mem::take(
        world
            .resources
            .get_or_init_mut::<steady_cache::TerrainQueueScratch>(),
    );
    let copies_non_empty =
        queue_scratch.populate_copies(world.resources.get::<surface_covering::SurfaceCopies>());
    let dem_arrival_epoch = crate::terrain::dem_arrival_epoch(world);
    let ctx = uniforms::surface_context(world, style, view_state, dem);
    let key_fn = draw_order_key_with_resolved(
        world
            .resources
            .get::<ResolvedFrameView>()
            .filter(|resolved| resolved.matches(style, view_state)),
        style,
        view_state,
        f64::from(dem.tile_size),
    );
    let result = (|| -> SystemResult {
        let Some(Initialized(terrain)) = world.resources.get_mut::<Eventually<TerrainResources>>()
        else {
            return Err(SystemError::Dependencies);
        };
        terrain.begin_dem_frame();
        uniforms::prepare_tiles_into(
            &mut queue_scratch.prepared,
            targets,
            terrain,
            &world.tiles,
            ctx,
        );
        uniforms::with_world_copies(
            &mut queue_scratch.prepared,
            copies_non_empty.then_some(&queue_scratch.copies_scratch),
            view_state,
        );
        let (edge_gen, written) = prepare_and_upload_tile_gpu(
            terrain,
            &mut edge_cache,
            &mut queue_scratch.prepared,
            &world.tiles,
            queue,
            dem_arrival_epoch,
            targets.0.len(),
        );
        queue_scratch.sync_draws(device, terrain, written, &key_fn);
        finish_surface_and_eye_frame(
            world,
            &mut queue_scratch.prepared,
            &edge_cache,
            edge_gen,
            written,
        );
        Ok(())
    })();
    world.resources.insert(edge_cache);
    world.resources.insert(queue_scratch);
    result
}

fn prepare_and_upload_tile_gpu(
    terrain: &mut TerrainResources,
    edge_cache: &mut edges::EdgeCache,
    prepared: &mut uniforms::PreparedTerrain,
    tiles: &crate::tcs::tiles::Tiles,
    queue: &crate::render::upload_queue::UploadQueue,
    dem_arrival_epoch: u64,
    target_count: usize,
) -> (u64, usize) {
    let (edge_gen, fallback_edges) = edge_cache.assign_indices_with_epoch(
        &prepared.sources,
        &mut prepared.uniforms,
        tiles,
        terrain.edge_capacity(),
        Some(dem_arrival_epoch),
    );
    let dirty_rows = edge_cache.take_dirty_rows();
    if let Some(edges) = &fallback_edges {
        terrain.write_edges_if_stale(queue, 0, edges, &[]);
    } else {
        terrain.write_edges_if_stale(queue, edge_gen, edge_cache.packed_edges(), &dirty_rows);
    }
    let written = terrain.write_uniforms(queue, &prepared.uniforms);
    tracing::debug!(
        targets = target_count,
        dem_hits = prepared
            .sources
            .iter()
            .filter(|(dem, _, _)| dem.is_some())
            .count(),
        written,
        "terrain frame queued"
    );
    (edge_gen, written)
}

fn finish_surface_and_eye_frame(
    world: &mut World,
    prepared: &mut uniforms::PreparedTerrain,
    edge_cache: &edges::EdgeCache,
    edge_gen: u64,
    written: usize,
) {
    if let (Some(index), Some(written_sources)) = (
        world
            .resources
            .get_mut::<crate::terrain::TerrainCoverageIndex>(),
        prepared.sources.get(..written),
    ) {
        index.set_surface_edges_if_stale(written_sources, &edge_cache.samples, edge_gen);
    }
    if steady_cache::needs_eye_frame(world) {
        let kept = prepared
            .sources
            .iter()
            .zip(prepared.uniforms.iter())
            .take(written)
            .map(|((_, coords, _), uniform)| (*coords, *uniform))
            .collect();
        let mut globe_bases = std::mem::take(&mut prepared.globe_bases);
        globe_bases.truncate(written);
        world.resources.insert(TerrainEyeFrame(kept, globe_bases));
    }
}
