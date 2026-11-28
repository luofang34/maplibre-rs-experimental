//! Prepares drape selection, fingerprints, budgets, and per-frame GPU allocation for terrain tiles.

use super::{drape_phase::background_clear_color, readiness, spec_cache, targets};
#[cfg(feature = "raster")]
use crate::raster::resource::RasterResources;
use crate::{
    coords::WorldTileCoords,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        memory_budget::MemoryBudget,
        tile_view_pattern::WgpuTileViewPattern,
        view_state::ViewState,
    },
    schedule::system::SystemError,
    style::Style,
    tcs::world::World,
    terrain::{
        drape_cache::{fingerprint, SourceContent},
        drape_targets::TargetSpec,
        queue_system::{
            acquire::acquire_drapes, activity, cohort, steady_cache, surface_covering, DrapeBudget,
        },
        resources::TerrainResources,
    },
    vector::VectorBufferPool,
};

/// The GPU content of source tiles, read from the vector pool and the raster textures.
pub(crate) struct LoadedContent<'a> {
    pool: Option<&'a VectorBufferPool>,
    #[cfg(feature = "raster")]
    raster: Option<&'a RasterResources>,
    dem: Option<&'a crate::hillshade::HillshadeResources>,
}

impl SourceContent for LoadedContent<'_> {
    fn vector_layer_revision(
        &self,
        _coords: WorldTileCoords,
        _layer_id: crate::style::StyleLayerId,
        cached_revision: u64,
    ) -> Option<u64> {
        self.pool.map(|_| cached_revision)
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

pub(crate) fn loaded_content(world: &World) -> LoadedContent<'_> {
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

pub(super) struct PreparedDrapes {
    pub(super) redraw: Vec<bool>,
    pub(super) sources: Vec<Option<WorldTileCoords>>,
    pub(super) clear_color: wgpu::Color,
}

pub(super) fn prepare_drapes(
    specs: &[TargetSpec],
    style: &Style,
    view_state: &ViewState,
    world: &mut World,
    device: &wgpu::Device,
    drape_zoom: f64,
) -> Result<PreparedDrapes, SystemError> {
    let memory = world
        .resources
        .get::<MemoryBudget>()
        .copied()
        .unwrap_or_default();
    let clear_color = background_clear_color(style);
    let prep =
        compute_or_reuse_drape_prep(specs, style, view_state, world, clear_color, drape_zoom);
    let prints = prep.prints.as_slice();
    let ready = prep.ready.as_slice();
    let mut prefetch = std::mem::take(
        &mut world
            .resources
            .get_or_init_mut::<steady_cache::AcquireScratch>()
            .prefetch,
    );
    prefetch.clear();
    match world.resources.get::<targets::PrefetchDrapes>() {
        Some(ahead) if !ahead.0.is_empty() => {
            prefetch.extend(specs.iter().map(|spec| ahead.0.contains(&spec.coords)));
        }
        _ => prefetch.resize(specs.len(), false),
    }
    let camera_moved = crate::render::frame_signals::camera_moved(world, view_state);
    let (budget, limit) = drape_frame_budget(world, view_state, camera_moved);
    let view_textures = crate::terrain::drape_share::textures(world, memory);
    let coalesce_motion = cfg!(target_arch = "wasm32") && camera_moved;
    let (redraw, drape_sources, deferred, mut states) = acquire_and_ensure_scratch(
        specs,
        (prints, ready, &prefetch),
        (memory, view_textures),
        (budget, limit),
        coalesce_motion,
        world,
        device,
    )?;
    record_drape_activity(
        world,
        &activity::FrameDrapes {
            specs,
            states: &states,
            ready,
            prefetch: &prefetch,
            redraw: &redraw,
            sources: &drape_sources,
            budget: (budget, limit),
            paint_zoom: drape_zoom,
        },
        deferred,
    );
    prefetch.clear();
    states.clear();
    let scratch = world
        .resources
        .get_or_init_mut::<steady_cache::AcquireScratch>();
    scratch.prefetch = prefetch;
    scratch.states = states;
    Ok(PreparedDrapes {
        redraw,
        sources: drape_sources,
        clear_color,
    })
}

type AcquiredDrapes = (
    Vec<bool>,
    Vec<Option<WorldTileCoords>>,
    bool,
    Vec<crate::terrain::drape_cache::DrapeState>,
);

fn acquire_and_ensure_scratch(
    specs: &[TargetSpec],
    (prints, ready, prefetch): (&[u64], &[bool], &[bool]),
    (memory, view_textures): (MemoryBudget, usize),
    (budget, limit): (usize, usize),
    coalesce_motion: bool,
    world: &mut World,
    device: &wgpu::Device,
) -> Result<AcquiredDrapes, SystemError> {
    if world
        .resources
        .get::<steady_cache::DrapeCoalesceState>()
        .is_none()
    {
        world
            .resources
            .insert(steady_cache::DrapeCoalesceState::default());
    }
    let Some((Initialized(terrain), coalesce, scratch)) = world.resources.query_mut::<(
        &mut Eventually<TerrainResources>,
        &mut steady_cache::DrapeCoalesceState,
        &mut steady_cache::AcquireScratch,
    )>() else {
        return Err(SystemError::Dependencies);
    };
    let acquired = acquire_drapes(
        specs,
        (prints, ready, prefetch),
        (memory, view_textures),
        (budget, limit),
        terrain,
        device,
        (coalesce, scratch, coalesce_motion),
    );
    for (spec, drawn) in specs.iter().zip(&acquired.0) {
        if *drawn {
            if let Some(edge_px) = terrain.drape_edge_px(spec.coords) {
                terrain.ensure_scratch_for_size(device, edge_px);
            }
        }
    }
    Ok(acquired)
}

fn record_drape_activity(world: &mut World, drapes: &activity::FrameDrapes<'_>, deferred: bool) {
    let drawn = |ahead: bool| {
        drapes
            .redraw
            .iter()
            .zip(drapes.prefetch)
            .filter(|(drawn, prefetched)| **drawn && **prefetched == ahead)
            .count()
    };
    crate::render::frame_signals::count_drape_redraws(world, drawn(false), drawn(true));
    activity::record(world, drapes);
    // A drape left for a later frame's budget needs that frame.
    if deferred {
        crate::render::frame_signals::keep_animating(world);
    }
}

struct PrepDigests {
    vector_pool_state: (u64, u64, bool),
    vector_arrival_epoch: u64,
    raster_arrival_epoch: u64,
    target_tiles_digest: u64,
    relevant_tiles_len: (usize, usize),
    raster_gen: u64,
    style_rev: u64,
    style_vis_digest: u64,
}

fn resolve_prep_digests(
    specs: &[TargetSpec],
    style: &Style,
    style_zoom: f64,
    host_frame: u64,
    world: &mut World,
) -> Option<(u64, PrepDigests)> {
    let cache = world.resources.get::<spec_cache::DrapePrepareCache>()?;
    let specs_gen = cache.specs_gen_for(specs)?;
    if let Some(d) = cache.matching_digests(host_frame, specs, style.revision()) {
        return Some((
            specs_gen,
            PrepDigests {
                vector_pool_state: d.vector_pool_state,
                vector_arrival_epoch: d.vector_arrival_epoch,
                raster_arrival_epoch: d.raster_arrival_epoch,
                target_tiles_digest: d.target_tiles_digest,
                relevant_tiles_len: d.relevant_tiles_len,
                raster_gen: d.raster_gen,
                style_rev: d.style_rev,
                style_vis_digest: d.style_vis_digest,
            },
        ));
    }
    let vector_pool_state = spec_cache::vector_pool_state(world);
    let vector_arrival_epoch = crate::vector::arrival_epoch(world);
    let raster_arrival_epoch = crate::tcs::tile_epochs::raster_arrival_epoch(world);
    let target_tiles_digest =
        spec_cache::target_coords_vector_digest(specs.iter().map(|spec| spec.coords), world);
    let relevant_tiles_len = spec_cache::relevant_tiles_len(world);
    let raster_gen = spec_cache::raster_mutation_gen(world);
    let style_rev = style.revision();
    let style_vis_digest = world
        .resources
        .get_or_init_mut::<spec_cache::DrapePrepareCache>()
        .style_visibility_digest_cached(style, style_zoom);
    Some((
        specs_gen,
        PrepDigests {
            vector_pool_state,
            vector_arrival_epoch,
            raster_arrival_epoch,
            target_tiles_digest,
            relevant_tiles_len,
            raster_gen,
            style_rev,
            style_vis_digest,
        },
    ))
}

fn build_prep_key(
    specs: &[TargetSpec],
    style: &Style,
    style_zoom: f64,
    world: &mut World,
    clear_color: wgpu::Color,
    drape_zoom: f64,
    strict: bool,
) -> Option<spec_cache::DrapePrepKey> {
    let host_frame = crate::render::frame_signals::host_frame_in_progress(world);
    let (specs_gen, digests) = resolve_prep_digests(specs, style, style_zoom, host_frame, world)?;
    Some(spec_cache::DrapePrepKey {
        specs_gen,
        drape_paint_zoom_bits: crate::tcs::paint_zoom::drape_paint_zoom(drape_zoom).to_bits(),
        clear_color_bits: [
            clear_color.r.to_bits(),
            clear_color.g.to_bits(),
            clear_color.b.to_bits(),
            clear_color.a.to_bits(),
        ],
        strict,
        vector_pool_state: digests.vector_pool_state,
        vector_arrival_epoch: digests.vector_arrival_epoch,
        raster_arrival_epoch: digests.raster_arrival_epoch,
        target_tiles_digest: digests.target_tiles_digest,
        relevant_tiles_len: digests.relevant_tiles_len,
        raster_gen: digests.raster_gen,
        hillshade_fingerprint: spec_cache::hillshade_fingerprint(world),
        style_rev: digests.style_rev,
        style_vis_digest: digests.style_vis_digest,
    })
}

fn compute_or_reuse_drape_prep(
    specs: &[TargetSpec],
    style: &Style,
    view_state: &ViewState,
    world: &mut World,
    clear_color: wgpu::Color,
    drape_zoom: f64,
) -> std::sync::Arc<spec_cache::CachedPrep> {
    // A tile whose vector sources are finished but not yet in the buffer pool would drape
    // blank; it shows an ancestor's drape until they are.
    let strict = world
        .resources
        .get::<cohort::TextureCohort>()
        .is_some_and(|state| state.enabled);
    let capacity = match world.resources.get::<Eventually<WgpuTileViewPattern>>() {
        Some(Initialized(pattern)) => pattern.remaining_metadata_capacity(),
        _ => 0,
    };
    let style_zoom = view_state.style_zoom().value();
    let prep_key = build_prep_key(
        specs,
        style,
        style_zoom,
        world,
        clear_color,
        drape_zoom,
        strict,
    );
    if let Some(cached) = prep_key.as_ref().and_then(|key| {
        world
            .resources
            .get::<spec_cache::DrapePrepareCache>()
            .and_then(|cache| cache.get_prep(key, capacity))
    }) {
        return cached;
    }
    let prints: Vec<u64> = {
        let content = loaded_content(world);
        specs
            .iter()
            .map(|spec| fingerprint(spec, &content, clear_color, drape_zoom))
            .collect()
    };
    let mut ready: Vec<bool> = specs
        .iter()
        .map(|spec| readiness::ready(spec, style, style_zoom, world, strict))
        .collect();
    let (needed_shapes, clipped) = surface_covering::fit_metadata(specs, &mut ready, capacity);
    world
        .resources
        .get_or_init_mut::<spec_cache::DrapePrepareCache>()
        .commit_prep(
            prep_key,
            prints,
            ready,
            (needed_shapes, clipped.then_some(capacity)),
        )
}

fn drape_frame_budget(world: &World, view_state: &ViewState, camera_moved: bool) -> (usize, usize) {
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
    let effective = if camera_moved {
        world
            .resources
            .get::<crate::terrain::drape_timing::DrapeCost>()
            .copied()
            .unwrap_or_default()
            .drapes_within(budget.moving_time, limit)
    } else {
        limit
    };
    (effective, limit)
}
