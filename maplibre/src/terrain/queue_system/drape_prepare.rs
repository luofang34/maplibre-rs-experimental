//! Prepares drape selection, fingerprints, budgets, and per-frame GPU allocation for terrain tiles.

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
    style::Style,
    tcs::{system::SystemError, world::World},
    terrain::{
        drape_cache::{fingerprint, SourceContent},
        drape_targets::TargetSpec,
        queue_system::{
            acquire::acquire_drapes, activity, cohort, drape_phase::background_clear_color,
            readiness, surface_covering, targets, DrapeBudget,
        },
        resources::TerrainResources,
    },
    vector::VectorBufferPool,
};

/// The GPU content of source tiles, read from the vector pool and the raster textures.
pub(super) struct LoadedContent<'a> {
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

pub(super) fn loaded_content(world: &World) -> LoadedContent<'_> {
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
    let prefetch: Vec<bool> = {
        let ahead = world.resources.get::<targets::PrefetchDrapes>();
        specs
            .iter()
            .map(|spec| ahead.is_some_and(|ahead| ahead.0.contains(&spec.coords)))
            .collect()
    };
    let (budget, limit) = drape_frame_budget(world, view_state);
    let view_textures = crate::terrain::drape_share::textures(world, memory);
    let coalesce_motion = cfg!(target_arch = "wasm32")
        && crate::render::frame_signals::camera_moved(world, view_state);
    if world
        .resources
        .get::<super::steady_cache::DrapeCoalesceState>()
        .is_none()
    {
        world
            .resources
            .insert(super::steady_cache::DrapeCoalesceState::default());
    }
    let (redraw, drape_sources, deferred, states) = acquire_and_ensure_scratch(
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
        (specs, &states, ready, &prefetch, &redraw, &drape_sources),
        (budget, limit),
        drape_zoom,
        deferred,
    );
    Ok(PreparedDrapes {
        redraw,
        sources: drape_sources,
        clear_color,
    })
}

fn acquire_and_ensure_scratch(
    specs: &[TargetSpec],
    (prints, ready, prefetch): (&[u64], &[bool], &[bool]),
    (memory, view_textures): (MemoryBudget, usize),
    (budget, limit): (usize, usize),
    coalesce_motion: bool,
    world: &mut World,
    device: &wgpu::Device,
) -> Result<
    (
        Vec<bool>,
        Vec<Option<WorldTileCoords>>,
        bool,
        Vec<crate::terrain::drape_cache::DrapeState>,
    ),
    SystemError,
> {
    let Some((Initialized(terrain), coalesce)) = world.resources.query_mut::<(
        &mut Eventually<TerrainResources>,
        &mut super::steady_cache::DrapeCoalesceState,
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
        (coalesce, coalesce_motion),
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

fn record_drape_activity(
    world: &mut World,
    (specs, states, ready, prefetch, redraw, drape_sources): (
        &[TargetSpec],
        &[crate::terrain::drape_cache::DrapeState],
        &[bool],
        &[bool],
        &[bool],
        &[Option<WorldTileCoords>],
    ),
    budget: (usize, usize),
    drape_zoom: f64,
    deferred: bool,
) {
    let drawn = |ahead: bool| {
        redraw
            .iter()
            .zip(prefetch)
            .filter(|(drawn, prefetched)| **drawn && **prefetched == ahead)
            .count()
    };
    crate::render::frame_signals::count_drape_redraws(world, drawn(false), drawn(true));
    activity::record(
        world,
        &activity::FrameDrapes {
            specs,
            states,
            ready,
            prefetch,
            redraw,
            sources: drape_sources,
            budget,
            paint_zoom: drape_zoom,
        },
    );
    // A drape left for a later frame's budget needs that frame.
    if deferred {
        crate::render::frame_signals::keep_animating(world);
    }
}

fn build_prep_key(
    specs: &[TargetSpec],
    style: &Style,
    style_zoom: f64,
    world: &World,
    clear_color: wgpu::Color,
    drape_zoom: f64,
) -> Option<super::spec_cache::DrapePrepKey> {
    use super::spec_cache;
    let strict = world
        .resources
        .get::<cohort::TextureCohort>()
        .is_some_and(|state| state.enabled);
    let capacity = match world.resources.get::<Eventually<WgpuTileViewPattern>>() {
        Some(Initialized(pattern)) => pattern.remaining_metadata_capacity(),
        _ => 0,
    };
    let host_frame = crate::render::frame_signals::host_frame_in_progress(world);
    let cache = world.resources.get::<spec_cache::DrapePrepareCache>()?;
    let specs_gen = cache.specs_gen_for(specs)?;
    let (
        vector_pool_state,
        vector_arrival_epoch,
        target_tiles_digest,
        tiles_len,
        raster_gen,
        style_rev,
        style_vis_digest,
    ) = match cache.matching_digests(host_frame, specs, style.revision()) {
        Some(d) => (
            d.vector_pool_state,
            d.vector_arrival_epoch,
            d.target_tiles_digest,
            d.tiles_len,
            d.raster_gen,
            d.style_rev,
            d.style_vis_digest,
        ),
        None => (
            spec_cache::vector_pool_state(world),
            crate::vector::arrival_epoch(world),
            spec_cache::target_coords_vector_digest(specs.iter().map(|spec| spec.coords), world),
            world.tiles.tiles.len(),
            spec_cache::raster_mutation_gen(world),
            style.revision(),
            spec_cache::style_visibility_digest(style, style_zoom),
        ),
    };
    Some(spec_cache::DrapePrepKey {
        specs_gen,
        drape_paint_zoom_bits: crate::vector::upload_system::drape_paint_zoom(drape_zoom).to_bits(),
        clear_color_bits: [
            clear_color.r.to_bits(),
            clear_color.g.to_bits(),
            clear_color.b.to_bits(),
            clear_color.a.to_bits(),
        ],
        strict,
        capacity,
        vector_pool_state,
        vector_arrival_epoch,
        target_tiles_digest,
        tiles_len,
        raster_gen,
        hillshade_fingerprint: spec_cache::hillshade_fingerprint(world),
        style_rev,
        style_vis_digest,
    })
}

fn compute_or_reuse_drape_prep(
    specs: &[TargetSpec],
    style: &Style,
    view_state: &ViewState,
    world: &mut World,
    clear_color: wgpu::Color,
    drape_zoom: f64,
) -> std::sync::Arc<super::spec_cache::CachedPrep> {
    use super::spec_cache;
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
    let prep_key = build_prep_key(specs, style, style_zoom, world, clear_color, drape_zoom);
    if let Some(cached) = prep_key.as_ref().and_then(|key| {
        world
            .resources
            .get::<spec_cache::DrapePrepareCache>()
            .and_then(|cache| cache.get_prep(key))
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
    surface_covering::fit_metadata(specs, &mut ready, capacity);
    let built = std::sync::Arc::new(spec_cache::CachedPrep { prints, ready });
    if let Some(key) = prep_key {
        world
            .resources
            .get_or_init_mut::<spec_cache::DrapePrepareCache>()
            .store_prep(key, std::sync::Arc::clone(&built));
    }
    built
}

fn drape_frame_budget(world: &World, view_state: &ViewState) -> (usize, usize) {
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
    let effective = if crate::render::frame_signals::camera_moved(world, view_state) {
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
