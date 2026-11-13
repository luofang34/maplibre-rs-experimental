//! Uploads data to the GPU which is needed for rendering.
use crate::{
    context::MapContext,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        resolved_view::ResolvedFrameView,
        tile_mesh::{GlobeTileMeshCache, TileMeshUsage},
        tile_view_pattern::WgpuTileViewPattern,
        Renderer,
    },
    tcs::system::{SystemError, SystemResult},
};

pub fn upload_system(
    MapContext {
        world,
        style,
        view_state,
        renderer: Renderer { device, queue, .. },
        ..
    }: &mut MapContext,
) -> SystemResult {
    let background_tiles = crate::background::queue_system::globe_tiles(world);
    let (view_proj, projection_data) = match world
        .resources
        .get::<ResolvedFrameView>()
        .filter(|cached| cached.matches(style, view_state))
    {
        Some(cached) => (cached.gpu_view_projection, cached.shader_projection),
        None => {
            let resolved = ResolvedFrameView::resolve(style, view_state).map_err(|error| {
                tracing::error!(%error, "unable to prepare projection state");
                SystemError::Setup
            })?;
            let snapshot = (resolved.gpu_view_projection, resolved.shader_projection);
            world.resources.insert(resolved);
            snapshot
        }
    };

    let Some((Initialized(tile_view_pattern), Initialized(projection_resources), tile_mesh_cache)) =
        world.resources.query_mut::<(
            &mut Eventually<WgpuTileViewPattern>,
            &mut Eventually<ProjectionGpuResources>,
            &mut GlobeTileMeshCache,
        )>()
    else {
        return Err(SystemError::Dependencies);
    };

    tile_view_pattern.upload_pattern(
        queue,
        &view_proj,
        view_state.width() as f32,
        view_state.height() as f32,
        view_state.style_zoom(),
    );
    projection_resources.upload(queue, projection_data);
    tile_mesh_cache.set_globe_active(projection_data.transition > 0.0);

    if projection_data.transition > 0.0 {
        for coords in background_tiles {
            tile_mesh_cache
                .prepare(device, queue, coords, TileMeshUsage::Raster, false)
                .map_err(|error| {
                    tracing::error!(%error, %coords, "unable to prepare globe background mesh");
                    SystemError::Setup
                })?;
        }
    }

    let mut visible_tiles = Vec::new();
    for view_tile in tile_view_pattern.iter() {
        view_tile.render(|shape| visible_tiles.push(shape.coords()));
    }
    for coords in visible_tiles {
        for (usage, generate_borders) in [
            (TileMeshUsage::Stencil, true),
            (TileMeshUsage::Stencil, false),
            (TileMeshUsage::Raster, true),
            (TileMeshUsage::Raster, false),
        ] {
            tile_mesh_cache
                .prepare(device, queue, coords, usage, generate_borders)
                .map_err(|error| {
                    tracing::error!(%error, %coords, "unable to prepare globe tile mesh");
                    SystemError::Setup
                })?;
        }
    }

    Ok(())
}
