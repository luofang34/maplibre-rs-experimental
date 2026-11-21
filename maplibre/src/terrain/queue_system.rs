//! Builds the drape targets and terrain draws for the current frame.

use crate::{
    context::MapContext,
    coords::WorldTileCoords,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        eye_covering::{drawn_covering, EyeInFrame},
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
        drape_targets::{
            collect_layer_specs, select_targets, select_targets_as, Arrivals, TargetSpec,
        },
        resources::{TerrainDraw, TerrainResources, UNIFORM_STRIDE},
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
mod targets;
#[cfg(any(test, feature = "headless"))]
pub(crate) use targets::target_specs;
pub(crate) use targets::{draw_order_key, procedural_specs, target_specs_arc};
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
    world.resources.insert(TerrainEyeFrame::default());
    world
        .resources
        .insert(surface_covering::SurfaceTiles::default());
    let (phase, surface_specs, surface_sources) = if drape_mode.is_procedural_relief() {
        let specs = procedural_specs(style, view_state, world)?;
        let sources = vec![None; specs.len()];
        (DrapePhase::default(), specs, sources)
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
        let (surface_specs, surface_sources) = surface_covering::for_frame(world, &specs, &sources);
        (phase, surface_specs, surface_sources)
    };
    queue_tiles(
        world,
        style,
        view_state,
        device,
        queue,
        &dem,
        (&surface_specs, &surface_sources),
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
    targets: (&[TargetSpec], &[Option<WorldTileCoords>]),
) -> SystemResult {
    let mut edge_cache = std::mem::take(world.resources.get_or_init_mut::<edges::EdgeCache>());
    let copies = world
        .resources
        .get::<surface_covering::SurfaceCopies>()
        .map(|copies| copies.0.clone());
    let ctx = uniforms::surface_context(world, style, view_state, dem);
    {
        let Some(Initialized(terrain)) = world.resources.get_mut::<Eventually<TerrainResources>>()
        else {
            return Err(SystemError::Dependencies);
        };
        let uniforms::PreparedTerrain {
            mut uniforms,
            sources,
        } = uniforms::prepare_tiles(targets, terrain, &world.tiles, ctx);
        let (edge_gen, fallback_edges) = edge_cache.assign_indices(
            &sources,
            &mut uniforms,
            &world.tiles,
            terrain.edge_capacity(),
        );
        let (sources, placed) =
            uniforms::with_world_copies(&mut uniforms, sources, copies.as_ref(), view_state);
        if let Some(edges) = &fallback_edges {
            terrain.write_edges_if_stale(queue, 0, edges);
        } else {
            terrain.write_edges_if_stale(queue, edge_gen, edge_cache.packed_edges());
        }
        let written = terrain.write_uniforms(queue, &uniforms);
        tracing::debug!(
            targets = targets.0.len(),
            dem_hits = sources.iter().filter(|(dem, _, _)| dem.is_some()).count(),
            written,
            "terrain frame queued"
        );
        let draws = sources
            .iter()
            .take(written)
            .enumerate()
            .filter_map(|(index, (dem_coords, coords, drape_source))| {
                Some(TerrainDraw {
                    coords: *coords,
                    bind_group: terrain.tile_bind_group(device, *dem_coords, *drape_source)?,
                    uniform_offset: (index as u64 * UNIFORM_STRIDE) as u32,
                })
            })
            .collect::<Vec<_>>();
        let key = targets::draw_order_key(style, view_state, f64::from(dem.tile_size));
        let mut draws = draws;
        draws.sort_by(|left, right| key(&left.coords).total_cmp(&key(&right.coords)));
        terrain.set_draws(draws);
        if let Some(index) = world
            .resources
            .get_mut::<crate::terrain::TerrainCoverageIndex>()
        {
            index.set_surface_edges(
                &sources[..written],
                std::sync::Arc::clone(&edge_cache.samples),
            );
        }
        let kept = placed.into_iter().zip(uniforms).take(written).collect();
        world.resources.insert(TerrainEyeFrame(kept));
    }
    world.resources.insert(edge_cache);

    Ok(())
}
