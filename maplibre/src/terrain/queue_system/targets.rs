//! Selects terrain textures and records their independent surface meshes.
use super::*;
use crate::{
    render::{
        eye_covering::drawn_covering, memory_budget::MemoryBudget,
        tile_view_pattern::RasterCoverings as SliceRasterCoverings,
    },
    terrain::drape_targets::{
        collect_layer_specs, select_targets, select_targets_as, Arrivals, TargetSpec,
    },
};

/// The tiles a frame drapes ahead of the view: prepared, never redrawn, and drawn only with
/// the time the view's own drapes leave.
#[derive(Default)]
pub(super) struct PrefetchDrapes(pub(super) rustc_hash::FxHashSet<WorldTileCoords>);

pub(crate) fn uses_uniform_texture_covering(view_state: &ViewState) -> bool {
    use cgmath::InnerSpace;
    view_state.external_globe_eye().is_some_and(|eye| {
        (eye.position.magnitude() - 1.0) * view_state.body().radius_meters >= 60_000.0
    })
}

#[cfg(any(test, feature = "headless"))]
pub(crate) fn target_specs(
    style: &Style,
    view_state: &ViewState,
    world: &mut World,
) -> Result<Vec<TargetSpec>, SystemError> {
    target_specs_arc(style, view_state, world).map(|specs| (*specs).clone())
}

pub(crate) fn target_specs_arc(
    style: &Style,
    view_state: &ViewState,
    world: &mut World,
) -> Result<std::sync::Arc<Vec<TargetSpec>>, SystemError> {
    let zoom = view_state.zoom();
    let (_, raster_coverings) =
        drawn_covering(style, view_state, world, zoom.zoom_level(DEFAULT_TILE_SIZE)).map_err(
            |error| {
                tracing::error!(%error, "unable to select terrain tiles");
                SystemError::Setup
            },
        )?;
    let Some(dem) = crate::terrain::source::dem_source(style) else {
        return Ok(std::sync::Arc::new(Vec::new()));
    };
    let Some(view_region) = crate::terrain::coverage::take_or_compute_tight_terrain_region(
        style,
        view_state,
        world,
        dem.tile_size,
    )
    .map_err(|error| {
        tracing::error!(%error, "unable to select terrain tiles");
        SystemError::Setup
    })?
    else {
        return Ok(std::sync::Arc::new(Vec::new()));
    };
    let (tiles, uniform_globe) = select_frame_drape_tiles(style, view_state, world, &view_region);
    let arrivals_over_standin = view_state.has_external_view() || view_state.pose_view().is_some();
    let (fresh_prefetch, prefetch_plan) = plan_prefetch_drapes(
        style,
        view_state,
        world,
        &tiles,
        uniform_globe || arrivals_over_standin,
    );
    let mut digests = build_frame_digests(style, view_state, world, &tiles, &fresh_prefetch);
    let raster_coverings_digest = super::spec_cache::raster_coverings_digest(&raster_coverings);
    let loose_raster_digest = prefetch_plan.as_ref().map_or(0, |(_, loose)| {
        super::spec_cache::raster_coverings_digest(loose)
    });
    let specs = build_or_reuse_specs(
        style,
        view_state.style_zoom().value(),
        world,
        (tiles, fresh_prefetch, &raster_coverings, prefetch_plan),
        (
            arrivals_over_standin,
            uniform_globe,
            raster_coverings_digest,
            loose_raster_digest,
        ),
        &mut digests,
    );
    world
        .resources
        .get_or_init_mut::<super::spec_cache::DrapePrepareCache>()
        .store_frame_digests(digests);
    Ok(specs)
}

fn build_or_reuse_specs(
    style: &Style,
    style_zoom: f64,
    world: &mut World,
    (tiles, fresh_prefetch, raster_coverings, prefetch_plan): (
        Vec<WorldTileCoords>,
        Vec<WorldTileCoords>,
        &SliceRasterCoverings,
        Option<PrefetchPlan>,
    ),
    scalars: (bool, bool, u64, u64),
    digests: &mut super::spec_cache::FrameTerrainDigests,
) -> std::sync::Arc<Vec<TargetSpec>> {
    use super::spec_cache;
    let (arrivals_over_standin, uniform_globe, raster_coverings_digest, loose_raster_digest) =
        scalars;
    if let Some((cached_specs, cached_prefetch)) = world
        .resources
        .get::<spec_cache::DrapePrepareCache>()
        .and_then(|cache| cache.get_specs_matching(&tiles, &fresh_prefetch, scalars, digests))
    {
        digests.specs_ptr = std::sync::Arc::as_ptr(&cached_specs) as usize;
        if !cached_prefetch.is_empty() {
            world
                .resources
                .insert(PrefetchDrapes(cached_prefetch.clone()));
        }
        return cached_specs;
    }
    let arrivals = if arrivals_over_standin {
        Arrivals::OverStandIn
    } else {
        Arrivals::Together
    };
    let raster_vec = raster_coverings.to_vec();
    let mut targets = select_targets_as(tiles.iter().copied(), world, &raster_vec, arrivals);
    let mut prefetch_drapes = rustc_hash::FxHashSet::default();
    if let Some((fresh, loose_raster)) = prefetch_plan {
        if !fresh.is_empty() {
            prefetch_drapes = fresh.iter().copied().collect();
            world
                .resources
                .insert(PrefetchDrapes(prefetch_drapes.clone()));
            targets.extend(select_targets(fresh.into_iter(), world, &loose_raster));
        }
    }
    let specs = std::sync::Arc::new(collect_layer_specs(targets, style, world, style_zoom));
    digests.specs_ptr = std::sync::Arc::as_ptr(&specs) as usize;
    let specs_key = spec_cache::TargetSpecsKey {
        tiles,
        prefetch_fresh: fresh_prefetch,
        arrivals_over_standin,
        uniform_globe,
        raster_coverings_digest,
        loose_raster_digest,
        vector_pool_state: digests.vector_pool_state,
        vector_arrival_epoch: digests.vector_arrival_epoch,
        target_tiles_digest: digests.target_tiles_digest,
        tiles_len: digests.tiles_len,
        raster_gen: digests.raster_gen,
        style_rev: digests.style_rev,
        style_vis_digest: digests.style_vis_digest,
    };
    world
        .resources
        .get_or_init_mut::<spec_cache::DrapePrepareCache>()
        .store_specs(specs_key, std::sync::Arc::clone(&specs), prefetch_drapes);
    specs
}

fn select_frame_drape_tiles(
    style: &Style,
    view_state: &ViewState,
    world: &mut World,
    view_region: &crate::coords::ViewRegion,
) -> (Vec<WorldTileCoords>, bool) {
    let memory = world
        .resources
        .get::<MemoryBudget>()
        .copied()
        .unwrap_or_default();
    world
        .resources
        .insert(surface_covering::SurfaceTiles(view_region.iter().collect()));
    world
        .resources
        .insert(surface_covering::SurfaceCopies::of(view_region.copies()));
    let uniform_globe = uses_uniform_texture_covering(view_state);
    world
        .resources
        .get_or_init_mut::<cohort::TextureCohort>()
        .enabled = uniform_globe;
    let allowance =
        crate::terrain::drape_share::allowance(world, memory, view_region.iter().count());
    let tiles: Vec<_> = if uniform_globe {
        cohort::targets(
            world,
            &view_region.iter().collect::<Vec<_>>(),
            allowance.textures,
        )
    } else if view_state.has_external_view() || view_state.pose_view().is_some() {
        // An eye's screen-resolution covering reaches its horizon in more tiles than the
        // textures allow; the surfaces keep every tile, so only the drapes coarsen.
        let focus = covering::focus_for_view(style, view_state);
        covering::for_frame(
            world,
            view_region.iter().collect(),
            // The view's share of the textures, less those it regroups with.
            allowance.targets,
            focus,
        )
    } else {
        view_region.iter().collect()
    };
    super::prefetch::prepare(
        style,
        view_state,
        world,
        &tiles,
        (memory, allowance.targets),
    );
    let had_new_drape = match world
        .resources
        .get::<crate::terrain::request_system::DrapeRequests>()
    {
        Some(prev) => tiles.iter().any(|tile| !prev.0.contains(tile)),
        None => !tiles.is_empty(),
    };
    if had_new_drape {
        crate::render::frame_signals::keep_animating(world);
    }
    world
        .resources
        .insert(crate::terrain::request_system::DrapeRequests(tiles.clone()));
    world.resources.insert(PrefetchDrapes::default());
    (tiles, uniform_globe)
}

type PrefetchPlan = (
    Vec<WorldTileCoords>,
    crate::render::eye_covering::RasterCoverings,
);

fn cached_loose_raster_regions(
    style: &Style,
    view_state: &ViewState,
    world: &mut World,
) -> Option<crate::render::eye_covering::RasterCoverings> {
    let host_frame = crate::render::frame_signals::host_frame_in_progress(world);
    let terrain_key = crate::render::eye_covering::terrain_index_key(world);
    if let Some(cached) = world
        .resources
        .get::<crate::render::eye_covering::FrameLooseCovering>()
        .filter(|cache| cache.host_frame == host_frame)
        .and_then(|cache| cache.raster_regions.as_ref())
        .filter(|(_, padding, cached_terrain)| {
            *padding == crate::render::view_state::ViewStatePadding::Loose
                && *cached_terrain == terrain_key
        })
        .map(|(regions, _, _)| regions.clone())
    {
        return Some(cached);
    }
    let vp_bits = crate::render::eye_covering::frame_view_proj_bits(world, style, view_state);
    let cache_key = (vp_bits, terrain_key, style.revision());
    if let Some(cached) = world
        .resources
        .get::<super::spec_cache::DrapePrepareCache>()
        .and_then(|cache| cache.get_loose_raster(cache_key))
    {
        return Some(cached);
    }
    let loose_raster = crate::render::projection::raster_source_regions(
        style,
        view_state,
        world,
        crate::render::view_state::ViewStatePadding::Loose,
    )
    .ok()?;
    world
        .resources
        .get_or_init_mut::<super::spec_cache::DrapePrepareCache>()
        .store_loose_raster(cache_key, loose_raster.clone());
    Some(loose_raster)
}

fn plan_prefetch_drapes(
    style: &Style,
    view_state: &ViewState,
    world: &mut World,
    tiles: &[WorldTileCoords],
    skip: bool,
) -> (Vec<WorldTileCoords>, Option<PrefetchPlan>) {
    if skip {
        return (Vec::new(), None);
    }
    let prefetch_tiles = world
        .resources
        .get::<crate::terrain::request_system::DrapePrefetchRequests>()
        .map(|requests| requests.0.clone())
        .unwrap_or_default();
    if prefetch_tiles.is_empty() {
        return (Vec::new(), None);
    }
    let Some(loose_raster) = cached_loose_raster_regions(style, view_state, world) else {
        return (Vec::new(), None);
    };
    let draped: rustc_hash::FxHashSet<WorldTileCoords> = tiles.iter().copied().collect();
    let held =
        |coords: &WorldTileCoords| match world.resources.get::<Eventually<TerrainResources>>() {
            Some(Eventually::Initialized(terrain)) => terrain.holds_shown_drape(*coords),
            _ => false,
        };
    let fresh_prefetch: Vec<_> = prefetch_tiles
        .into_iter()
        .filter(|coords| !draped.contains(coords) && !held(coords))
        .collect();
    let plan = Some((fresh_prefetch.clone(), loose_raster));
    (fresh_prefetch, plan)
}

fn build_frame_digests(
    style: &Style,
    view_state: &ViewState,
    world: &World,
    tiles: &[WorldTileCoords],
    fresh_prefetch: &[WorldTileCoords],
) -> super::spec_cache::FrameTerrainDigests {
    use super::spec_cache;
    let style_zoom = view_state.style_zoom().value();
    let target_tiles_digest = spec_cache::target_coords_vector_digest(
        tiles.iter().chain(fresh_prefetch.iter()).copied(),
        world,
    );
    spec_cache::FrameTerrainDigests {
        host_frame: crate::render::frame_signals::host_frame_in_progress(world),
        specs_ptr: 0,
        vector_pool_state: spec_cache::vector_pool_state(world),
        vector_arrival_epoch: crate::vector::arrival_epoch(world),
        target_tiles_digest,
        tiles_len: world.tiles.tiles.len(),
        raster_gen: spec_cache::raster_mutation_gen(world),
        style_rev: style.revision(),
        style_vis_digest: spec_cache::style_visibility_digest(style, style_zoom),
    }
}

/// Selects pure 3D DEM surface tile coordinates for
/// [`crate::render::settings::TerrainDrapeMode::ProceduralRelief`] without computing 2D drapes.
pub(crate) fn procedural_tiles(
    style: &Style,
    view_state: &ViewState,
    world: &mut World,
) -> Result<Vec<WorldTileCoords>, SystemError> {
    let Some(dem) = crate::terrain::source::dem_source(style) else {
        return Ok(Vec::new());
    };
    let view_region = crate::terrain::coverage::take_or_compute_tight_terrain_region(
        style,
        view_state,
        world,
        dem.tile_size,
    )
    .map_err(|error| {
        tracing::error!(%error, "unable to select procedural terrain tiles");
        SystemError::Setup
    })?;
    let Some(view_region) = view_region else {
        return Ok(Vec::new());
    };
    let tiles: Vec<_> = view_region.iter().collect();
    world
        .resources
        .insert(surface_covering::SurfaceTiles(tiles.clone()));
    world
        .resources
        .insert(surface_covering::SurfaceCopies::of(view_region.copies()));
    world
        .resources
        .get_or_init_mut::<cohort::TextureCohort>()
        .enabled = false;
    world
        .resources
        .insert(crate::terrain::request_system::DrapePrefetchRequests::default());
    world
        .resources
        .insert(crate::terrain::request_system::DrapeRequests::default());
    Ok(tiles)
}

/// The key terrain tiles are drawn by, smallest first: `1.0 - c_hat.dot(focus_unit)` on `S^2`
/// under Globe rendering, and GL JS's planar `(dx, dy)` squared distance at the nominal level
/// under explicit 2D Mercator rendering. With translucent drapes the order decides which skirts
/// show through.
pub(crate) fn draw_order_key(
    style: &Style,
    view_state: &ViewState,
    dem_tile_size: f64,
) -> impl Fn(&WorldTileCoords) -> f64 + use<> {
    draw_order_key_with_resolved(None, style, view_state, dem_tile_size)
}

pub(crate) fn draw_order_key_with_resolved(
    resolved: Option<&crate::render::resolved_view::ResolvedFrameView>,
    style: &Style,
    view_state: &ViewState,
    dem_tile_size: f64,
) -> impl Fn(&WorldTileCoords) -> f64 + use<> {
    enum OrderMode {
        Spherical(cgmath::Vector3<f64>),
        Planar([f64; 2]),
    }
    let mode = resolved
        .and_then(|rv| match &rv.projection {
            crate::render::resolved_view::ProjectionMode::Globe { camera, .. } => {
                Some(OrderMode::Spherical(camera.focus_unit_sphere()))
            }
            crate::render::resolved_view::ProjectionMode::FlatMercator => None,
        })
        .or_else(|| {
            let uses_globe = style.projection.as_ref().is_some_and(|p| {
                p.projection_type
                    .uses_globe_rendering(view_state.zoom().value())
            });
            uses_globe
                .then(|| {
                    crate::render::projection::globe_camera_for_view(view_state)
                        .ok()
                        .map(|globe| OrderMode::Spherical(globe.focus_unit_sphere()))
                })
                .flatten()
        })
        .unwrap_or_else(|| {
            let request = crate::render::projection::CoveringRequest::terrain(
                view_state.zoom().value(),
                dem_tile_size,
            );
            let tiles_across = 2_f64.powi(i32::from(u8::from(request.level)));
            let world_size = crate::coords::TILE_SIZE * 2_f64.powf(view_state.zoom().value());
            let center = view_state.camera().position();
            OrderMode::Planar([
                center.x / world_size * tiles_across - 0.5,
                center.y / world_size * tiles_across - 0.5,
            ])
        });
    move |tile: &WorldTileCoords| match mode {
        OrderMode::Spherical(focus_unit) => {
            crate::projection::tile_covering::spherical_tile_distance(*tile, focus_unit)
        }
        OrderMode::Planar(center) => {
            let dx = center[0] - f64::from(tile.x);
            let dy = center[1] - f64::from(tile.y);
            dx * dx + dy * dy
        }
    }
}
