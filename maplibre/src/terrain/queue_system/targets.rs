//! Selects terrain textures and records their independent surface meshes.
use super::*;

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

pub(crate) fn target_specs(
    style: &Style,
    view_state: &ViewState,
    world: &mut World,
) -> Result<Vec<TargetSpec>, SystemError> {
    let zoom = view_state.zoom();
    let (_, raster_coverings) =
        drawn_covering(style, view_state, world, zoom.zoom_level(DEFAULT_TILE_SIZE)).map_err(
            |error| {
                tracing::error!(%error, "unable to select terrain tiles");
                SystemError::Setup
            },
        )?;
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
        tracing::error!(%error, "unable to select terrain tiles");
        SystemError::Setup
    })?;
    let Some(view_region) = view_region else {
        return Ok(Vec::new());
    };
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
    world
        .resources
        .insert(crate::terrain::request_system::DrapeRequests(tiles.clone()));
    world.resources.insert(PrefetchDrapes::default());
    let arrivals = if view_state.has_external_view() || view_state.pose_view().is_some() {
        Arrivals::OverStandIn
    } else {
        Arrivals::Together
    };
    let mut targets = select_targets_as(tiles.into_iter(), world, &raster_coverings, arrivals);
    if !uniform_globe && !view_state.has_external_view() && view_state.pose_view().is_none() {
        let prefetch_tiles = world
            .resources
            .get::<crate::terrain::request_system::DrapePrefetchRequests>()
            .map(|requests| requests.0.clone())
            .unwrap_or_default();
        if !prefetch_tiles.is_empty() {
            if let Ok(loose_raster) = crate::render::projection::raster_source_regions(
                style,
                view_state,
                world,
                crate::render::view_state::ViewStatePadding::Loose,
            ) {
                // Prefetch prepares drapes ahead of the view, so they are ready when their tile
                // scrolls in, and leaves alone those a view has drawn. A tile the view drapes
                // keeps its view spec, as a second spec from the looser prefetch covering would
                // alternate the drape's fingerprint and redraw it every frame, and a drape parked
                // when its tile left the view keeps the content its view drew for its return.
                let draped: rustc_hash::FxHashSet<WorldTileCoords> =
                    targets.iter().map(|(coords, _)| *coords).collect();
                let held = |coords: &WorldTileCoords| match world
                    .resources
                    .get::<Eventually<TerrainResources>>()
                {
                    Some(Eventually::Initialized(terrain)) => terrain.holds_shown_drape(*coords),
                    _ => false,
                };
                let fresh: Vec<WorldTileCoords> = prefetch_tiles
                    .into_iter()
                    .filter(|coords| !draped.contains(coords) && !held(coords))
                    .collect();
                world
                    .resources
                    .insert(PrefetchDrapes(fresh.iter().copied().collect()));
                targets.extend(select_targets(fresh.into_iter(), world, &loose_raster));
            }
        }
    }
    Ok(collect_layer_specs(
        targets,
        style,
        world,
        view_state.style_zoom().value(),
    ))
}

/// Selects pure 3D DEM surface tiles for [`crate::render::settings::TerrainDrapeMode::ProceduralRelief`]
/// without computing 2D drape targets or prefetching draped source tiles.
pub(crate) fn procedural_specs(
    style: &Style,
    view_state: &ViewState,
    world: &mut World,
) -> Result<Vec<TargetSpec>, SystemError> {
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
    Ok(tiles
        .into_iter()
        .map(|coords| TargetSpec {
            coords,
            shapes: Vec::new(),
            absent_sources: Vec::new(),
        })
        .collect())
}

/// The key terrain tiles are drawn by, smallest first: `1.0 - c_hat.dot(focus_unit)` on `S^2`
/// under Globe rendering, and GL JS's planar `(dx, dy)` squared distance at the nominal level
/// under explicit 2D Mercator rendering. With translucent drapes the order decides which skirts
/// show through.
pub(super) fn draw_order_key(
    style: &Style,
    view_state: &ViewState,
    dem_tile_size: f64,
) -> impl Fn(&WorldTileCoords) -> f64 + use<> {
    enum OrderMode {
        Spherical(cgmath::Vector3<f64>),
        Planar([f64; 2]),
    }
    let uses_globe = style.projection.as_ref().is_some_and(|p| {
        p.projection_type
            .uses_globe_rendering(view_state.zoom().value())
    });
    let mode = if uses_globe {
        crate::render::projection::globe_camera_for_view(view_state)
            .ok()
            .map(|globe| OrderMode::Spherical(globe.focus_unit_sphere()))
    } else {
        None
    }
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
