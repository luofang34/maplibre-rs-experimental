//! Extracts data from the current state.

use crate::{
    context::MapContext,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        eye_covering::drawn_covering,
        tile_view_pattern::{ViewTileSources, WgpuTileViewPattern, DEFAULT_TILE_SIZE},
    },
    tcs::system::{SystemError, SystemResult},
};

pub fn tile_view_pattern_system(
    MapContext {
        style,
        view_state,
        world,
        ..
    }: &mut MapContext,
) -> SystemResult {
    // Create the tile view pattern only for tiles in view -> Tight
    let (view_region, raster_coverings) = drawn_covering(
        style,
        view_state,
        world,
        view_state.zoom().zoom_level(DEFAULT_TILE_SIZE),
    )
    .map_err(|error| {
        tracing::error!(%error, "unable to select tiles for rendering");
        SystemError::Setup
    })?;
    let Some((Initialized(tile_view_pattern), view_tile_sources)) = world
        .resources
        .query::<(&Eventually<WgpuTileViewPattern>, &ViewTileSources)>()
    else {
        return Err(SystemError::Dependencies);
    };

    if let Some(view_region) = &view_region {
        let zoom = view_state.zoom();

        let view_tiles = tile_view_pattern.generate_pattern(
            view_region,
            view_tile_sources,
            &raster_coverings,
            zoom,
            world,
        );

        // TODO: Can we &mut borrow initially somehow instead of here?
        let Some(Initialized(tile_view_pattern)) = world
            .resources
            .query_mut::<&mut Eventually<WgpuTileViewPattern>>()
        else {
            return Err(SystemError::Dependencies);
        };

        log::trace!("Tiles in view: {}", view_tiles.len());

        tile_view_pattern.update_pattern(view_tiles);
    }

    Ok(())
}
