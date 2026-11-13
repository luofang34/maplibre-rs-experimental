//! Builds the drape targets and terrain draws for the current frame.

#[cfg(feature = "raster")]
use crate::raster::resource::RasterResources;
use crate::{
    context::MapContext,
    coords::WorldTileCoords,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        eye_covering::{drawn_covering, EyeInFrame},
        memory_budget::MemoryBudget,
        tile_view_pattern::{WgpuTileViewPattern, DEFAULT_TILE_SIZE},
        view_state::ViewState,
        Renderer,
    },
    style::Style,
    tcs::{
        system::{SystemError, SystemResult},
        world::World,
    },
    terrain::{
        drape_cache::{fingerprint, SourceContent},
        drape_targets::{
            collect_layer_specs, select_targets, select_targets_as, Arrivals, TargetSpec,
        },
        resources::{TerrainDraw, TerrainResources, UNIFORM_STRIDE},
        source::{dem_source, DemSource},
        DrapePhase, TerrainFrame,
    },
    vector::VectorBufferPool,
};
mod acquire;
mod activity;
#[cfg(feature = "headless")]
pub(crate) use activity::PendingDrapes;
mod cohort;
mod covering;
pub(crate) mod edges;
mod prefetch;
mod readiness;
mod targets;
pub(crate) use targets::{procedural_specs, target_specs};
mod drape_phase;
mod frame;
pub(crate) mod surface_covering;
pub(crate) mod uniforms;
use acquire::acquire_drapes;
use drape_phase::{background_clear_color, encode_drapes};
use frame::{hide_draped_layers, set_frame};
use uniforms::present_ancestors;
pub(crate) use uniforms::TerrainEyeFrame;

/// Divisor of the tile circumference giving the skirt drop, as in GL JS `getSkirtLength`.
const SKIRT_DIVISOR: f64 = 5.0;
/// Drape textures drawn in one frame. Every level of a drape's mip chain is a render pass,
/// and the Metal backend closes a command buffer per pass that stays open until the frame is
/// submitted, so a burst of arriving tiles must not open more than the queue holds; it also
/// keeps the frame's GPU time bounded while the rest wait a frame.
const MAX_DRAPES_PER_FRAME: usize = 24;
/// Drape textures drawn per frame while a host's eye drives the map. A flight lands hundreds
/// of tiles within a few frames, and a display's frame budget holds only a few drapes.
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
        Self {
            per_frame: MAX_DRAPES_PER_FRAME,
            per_eye_frame: EYE_DRAPES_PER_FRAME,
            moving_time: std::time::Duration::from_millis(4),
        }
    }
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
    let Some(dem) = dem_source(style) else {
        world.resources.insert(DrapePhase::default());
        set_frame(world, TerrainFrame::default());
        return Ok(());
    };
    let (phase, surface_specs, surface_sources) = if drape_mode.is_procedural_relief() {
        let specs = procedural_specs(style, view_state, world)?;
        let sources = vec![None; specs.len()];
        (DrapePhase::default(), specs, sources)
    } else {
        let specs = target_specs(style, view_state, world)?;
        let PreparedDrapes {
            redraw,
            sources,
            clear_color,
        } = prepare_drapes(&specs, style, view_state, world, device)?;
        let paint_zoom = crate::vector::upload_system::paint::current_zoom(
            world,
            view_state.style_zoom().value(),
        );
        let phase = encode_drapes(
            &specs,
            &redraw,
            clear_color,
            world,
            crate::coords::Zoom::new(paint_zoom),
            queue,
        )?;
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

/// The GPU content of source tiles, read from the vector pool and the raster textures.
struct LoadedContent<'a> {
    pool: Option<&'a VectorBufferPool>,
    #[cfg(feature = "raster")]
    raster: Option<&'a RasterResources>,
    dem: Option<&'a crate::hillshade::HillshadeResources>,
}

impl SourceContent for LoadedContent<'_> {
    fn vector_layer_revision(&self, coords: WorldTileCoords, layer_id: &str) -> Option<u64> {
        self.pool?
            .index()
            .get_layers(coords)?
            .iter()
            .find(|entry| entry.style_layer.id == layer_id)
            .map(|entry| entry.content_revision())
    }

    fn raster_revision(&self, coords: WorldTileCoords, layer: &str) -> Option<u64> {
        #[cfg(feature = "raster")]
        {
            self.raster
                .and_then(|raster| raster.texture_revision(layer, coords))
        }
        #[cfg(not(feature = "raster"))]
        {
            let _ = (coords, layer);
            None
        }
    }

    fn dem_layer_fingerprint(&self, layer: &str) -> Option<u64> {
        self.dem.and_then(|dem| dem.layer_fingerprint(layer))
    }
}

fn loaded_content(world: &World) -> LoadedContent<'_> {
    LoadedContent {
        pool: match world.resources.get::<Eventually<VectorBufferPool>>() {
            Some(Initialized(pool)) => Some(pool),
            _ => None,
        },
        #[cfg(feature = "raster")]
        raster: match world.resources.get::<Eventually<RasterResources>>() {
            Some(Initialized(raster)) => Some(raster),
            _ => None,
        },
        dem: match world
            .resources
            .get::<Eventually<crate::hillshade::HillshadeResources>>()
        {
            Some(Initialized(dem)) => Some(dem),
            _ => None,
        },
    }
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
        let edges = edge_cache.apply(&sources, &mut uniforms, &world.tiles);
        let (sources, placed) =
            uniforms::with_world_copies(&mut uniforms, sources, copies.as_ref(), view_state);
        terrain.write_edges(queue, &edges);
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
            index.set_surface_edges(&sources, std::sync::Arc::clone(&edge_cache.samples));
        }
        let kept = placed.into_iter().zip(uniforms).take(written).collect();
        world.resources.insert(TerrainEyeFrame(kept));
    }
    world.resources.insert(edge_cache);

    Ok(())
}

struct PreparedDrapes {
    redraw: Vec<bool>,
    sources: Vec<Option<WorldTileCoords>>,
    clear_color: wgpu::Color,
}

fn prepare_drapes(
    specs: &[TargetSpec],
    style: &Style,
    view_state: &ViewState,
    world: &mut World,
    device: &wgpu::Device,
) -> Result<PreparedDrapes, SystemError> {
    let memory = world
        .resources
        .get::<MemoryBudget>()
        .copied()
        .unwrap_or_default();
    let clear_color = background_clear_color(style);
    let paint_zoom =
        crate::vector::upload_system::paint::current_zoom(world, view_state.style_zoom().value());
    let prints: Vec<u64> = {
        let content = loaded_content(world);
        specs
            .iter()
            .map(|spec| fingerprint(spec, &content, clear_color, paint_zoom))
            .collect()
    };
    // A tile whose vector sources are finished but not yet in the buffer pool would drape
    // blank; it shows an ancestor's drape until they are.
    let strict = world
        .resources
        .get::<cohort::TextureCohort>()
        .is_some_and(|state| state.enabled);
    let mut ready: Vec<bool> = specs
        .iter()
        .map(|spec| readiness::ready(spec, style, view_state.style_zoom().value(), world, strict))
        .collect();
    let capacity = match world.resources.get::<Eventually<WgpuTileViewPattern>>() {
        Some(Initialized(pattern)) => pattern.remaining_metadata_capacity(),
        _ => 0,
    };
    surface_covering::fit_metadata(specs, &mut ready, capacity);
    let prefetch: Vec<bool> = {
        let ahead = world.resources.get::<targets::PrefetchDrapes>();
        specs
            .iter()
            .map(|spec| ahead.is_some_and(|ahead| ahead.0.contains(&spec.coords)))
            .collect()
    };
    let budget = world
        .resources
        .get::<DrapeBudget>()
        .copied()
        .unwrap_or_default();
    let limit = if view_state.has_external_view() {
        budget.per_eye_frame
    } else {
        budget.per_frame
    };
    // A moving view draws what fits its time; the view at rest draws the rest at full pace.
    let budget = if crate::render::frame_signals::camera_moved(world, view_state) {
        world
            .resources
            .get::<crate::terrain::drape_timing::DrapeCost>()
            .copied()
            .unwrap_or_default()
            .drapes_within(budget.moving_time, limit)
    } else {
        limit
    };
    let view_textures = crate::terrain::drape_share::textures(world, memory);
    let (redraw, drape_sources, deferred, states) = {
        let Some(Initialized(terrain)) = world.resources.get_mut::<Eventually<TerrainResources>>()
        else {
            return Err(SystemError::Dependencies);
        };
        terrain.ensure_scratch(device);
        acquire_drapes(
            specs,
            (&prints, &ready, &prefetch),
            (memory, view_textures),
            (budget, limit),
            terrain,
            device,
        )
    };
    let drawn = |ahead: bool| {
        redraw
            .iter()
            .zip(&prefetch)
            .filter(|(drawn, prefetched)| **drawn && **prefetched == ahead)
            .count()
    };
    crate::render::frame_signals::count_drape_redraws(world, drawn(false), drawn(true));
    activity::record(
        world,
        &activity::FrameDrapes {
            specs,
            states: &states,
            ready: &ready,
            prefetch: &prefetch,
            redraw: &redraw,
            sources: &drape_sources,
            budget: (budget, limit),
            paint_zoom,
        },
    );
    // A drape left for a later frame's budget needs that frame.
    if deferred {
        crate::render::frame_signals::keep_animating(world);
    }
    Ok(PreparedDrapes {
        redraw,
        sources: drape_sources,
        clear_color,
    })
}
