//! Extracts data from the current state.

use crate::{
    context::MapContext,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        eye_covering::drawn_covering,
        tile_view_pattern::{
            TileViewPatternMemo, ViewTileSources, WgpuTileViewPattern, DEFAULT_TILE_SIZE,
        },
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
    if let Some(view_region) = &view_region {
        let zoom = view_state.zoom();
        let style_rev = style.revision();
        let mut memo = world
            .resources
            .remove::<TileViewPatternMemo>()
            .unwrap_or_default();
        let Some(Initialized(tile_view_pattern)) =
            world.resources.get::<Eventually<WgpuTileViewPattern>>()
        else {
            world.resources.insert(memo);
            return Err(SystemError::Dependencies);
        };
        let is_empty = tile_view_pattern.is_empty();
        if !is_empty && memo.matches(view_region, &raster_coverings, style_rev, world) {
            if memo.needs_zoom_update(zoom) {
                if let Some(Initialized(tile_view_pattern)) =
                    world.resources.get_mut::<Eventually<WgpuTileViewPattern>>()
                {
                    tile_view_pattern.update_zoom_in_place(zoom);
                }
            }
            world.resources.insert(memo);
            return Ok(());
        }
        let Some((Initialized(tile_view_pattern), view_tile_sources)) = world
            .resources
            .query::<(&Eventually<WgpuTileViewPattern>, &ViewTileSources)>()
        else {
            world.resources.insert(memo);
            return Err(SystemError::Dependencies);
        };
        let view_tiles = tile_view_pattern.generate_pattern(
            view_region,
            view_tile_sources,
            &raster_coverings,
            zoom,
            world,
        );
        memo.record(view_region, &raster_coverings, style_rev, zoom, world);
        log::trace!("Tiles in view: {}", view_tiles.len());

        let Some(Initialized(tile_view_pattern)) = world
            .resources
            .query_mut::<&mut Eventually<WgpuTileViewPattern>>()
        else {
            world.resources.insert(memo);
            return Err(SystemError::Dependencies);
        };
        tile_view_pattern.update_pattern(view_tiles);
        world.resources.insert(memo);
    }

    Ok(())
}
