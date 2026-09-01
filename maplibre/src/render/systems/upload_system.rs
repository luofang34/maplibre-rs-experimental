//! Uploads data to the GPU which is needed for rendering.
use crate::{
    context::MapContext,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::{projection_data_for_view, ProjectionGpuResources},
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
        renderer: Renderer { queue, .. },
        ..
    }: &mut MapContext,
) -> SystemResult {
    let Some((Initialized(tile_view_pattern), Initialized(projection_resources))) =
        world.resources.query_mut::<(
            &mut Eventually<WgpuTileViewPattern>,
            &mut Eventually<ProjectionGpuResources>,
        )>()
    else {
        return Err(SystemError::Dependencies);
    };

    let view_proj = view_state.view_projection();
    tile_view_pattern.upload_pattern(
        queue,
        &view_proj,
        view_state.width() as f32,
        view_state.height() as f32,
    );
    let projection_data = projection_data_for_view(style, view_state).map_err(|error| {
        tracing::error!(%error, "unable to prepare projection state");
        SystemError::Setup
    })?;
    projection_resources.upload(queue, projection_data);

    Ok(())
}
