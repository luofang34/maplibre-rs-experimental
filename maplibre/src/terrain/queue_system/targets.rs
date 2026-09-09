//! Selects terrain textures and records their independent surface meshes.
use super::*;

pub(super) fn target_specs(
    style: &Style,
    view_state: &ViewState,
    world: &mut World,
) -> Result<Vec<TargetSpec>, SystemError> {
    let zoom = view_state.zoom();
    let (view_region, raster_coverings) =
        drawn_covering(style, view_state, world, zoom.zoom_level(DEFAULT_TILE_SIZE)).map_err(
            |error| {
                tracing::error!(%error, "unable to select terrain tiles");
                SystemError::Setup
            },
        )?;
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
    let uniform_globe = uses_uniform_texture_covering(view_state);
    world
        .resources
        .get_or_init_mut::<cohort::TextureCohort>()
        .enabled = uniform_globe;
    let tiles: Vec<_> = if uniform_globe {
        cohort::targets(
            world,
            &view_region.iter().collect::<Vec<_>>(),
            memory.drape_textures_allowed(),
        )
    } else if view_state.has_external_view() {
        covering::for_frame(
            world,
            view_region.iter().collect(),
            // A complete replacement must fit beside the presented coverage.
            (memory.drape_textures_allowed() / 2).max(1),
        )
    } else {
        view_region.iter().collect()
    };
    world
        .resources
        .insert(crate::terrain::request_system::DrapeRequests(tiles.clone()));
    let targets = select_targets(tiles.into_iter(), world, &raster_coverings);
    Ok(collect_layer_specs(targets, style, world, zoom.value()))
}
