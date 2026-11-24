//! Extracts data from the current state.

use crate::{
    context::MapContext,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        eye_covering::drawn_covering,
        tile_view_pattern::{
            compute_memo_key, TileViewPatternMemo, ViewTileSources, WgpuTileViewPattern,
            DEFAULT_TILE_SIZE,
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
        let next_key = compute_memo_key(view_region, &raster_coverings, style_rev, world);
        let hit = world
            .resources
            .query::<(&TileViewPatternMemo, &Eventually<WgpuTileViewPattern>)>()
            .map(|(memo, pattern)| {
                let (is_empty, drops_shapes) = match pattern {
                    Initialized(p) => (p.is_empty(), p.drops_shapes()),
                    _ => return Err(SystemError::Dependencies),
                };
                Ok((
                    !is_empty && memo.matches_key(next_key.as_ref(), drops_shapes),
                    memo.zoom_changed(zoom),
                ))
            })
            .transpose()?;
        if let Some((true, zoom_changed)) = hit {
            if zoom_changed
                && let Some((memo, Initialized(tile_view_pattern))) = world.resources.query_mut::<(
                    &mut TileViewPatternMemo,
                    &mut Eventually<WgpuTileViewPattern>,
                )>(
                )
            {
                memo.set_zoom(zoom);
                tile_view_pattern.update_zoom_in_place(zoom);
            }
            return Ok(());
        }
        let mut view_tiles = world
            .resources
            .get_mut::<TileViewPatternMemo>()
            .map_or_else(Vec::new, TileViewPatternMemo::take_scratch);
        let Some((Initialized(tile_view_pattern), view_tile_sources)) = world
            .resources
            .query::<(&Eventually<WgpuTileViewPattern>, &ViewTileSources)>()
        else {
            if let Some(memo) = world.resources.get_mut::<TileViewPatternMemo>() {
                memo.put_scratch(view_tiles);
            }
            return Err(SystemError::Dependencies);
        };
        tile_view_pattern.generate_pattern_into(
            view_region,
            view_tile_sources,
            &raster_coverings,
            zoom,
            world,
            &mut view_tiles,
        );
        log::trace!("Tiles in view: {}", view_tiles.len());

        let Some(Initialized(tile_view_pattern)) = world
            .resources
            .query_mut::<&mut Eventually<WgpuTileViewPattern>>()
        else {
            if let Some(memo) = world.resources.get_mut::<TileViewPatternMemo>() {
                memo.put_scratch(view_tiles);
            }
            return Err(SystemError::Dependencies);
        };
        tile_view_pattern.update_pattern_swapped(&mut view_tiles);
        let memo = world.resources.get_or_init_mut::<TileViewPatternMemo>();
        memo.put_scratch(view_tiles);
        memo.record_key(next_key, zoom);
    }

    Ok(())
}
