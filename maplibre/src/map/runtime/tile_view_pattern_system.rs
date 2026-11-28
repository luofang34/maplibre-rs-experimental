//! Extracts data from the current state.

use std::sync::Arc;

use crate::{
    context::MapContext,
    coords::{ViewRegion, WorldTileCoords, Zoom, ZoomLevel},
    io::tile_sources::RasterSourceId,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        eye_covering::{drawn_covering, matching_drawn_tiles_arc},
        tile_view_pattern::{
            compute_fast_memo_key, compute_memo_key, FastPatternKey, PatternEpochSnapshot,
            PatternMemoKey, TileViewPatternMemo, ViewTileSources, WgpuTileViewPattern,
            DEFAULT_TILE_SIZE,
        },
    },
    schedule::system::{SystemError, SystemResult},
    tcs::world::World,
    vector::{VectorBufferPool, VectorLayerBucketComponent},
};

fn capture_epoch_snapshot(world: &World, has_raster: bool) -> PatternEpochSnapshot {
    #[cfg(feature = "raster")]
    let has_active_cross_fade = has_raster
        && world
            .resources
            .get::<crate::raster::cross_fade::RasterCrossFade>()
            .is_some_and(|fade| !fade.is_empty());
    #[cfg(not(feature = "raster"))]
    let has_active_cross_fade = false;

    let (vector_pool_rev, vector_pool_eviction_gen, vector_pool_has_capacity) =
        match world.resources.get::<Eventually<VectorBufferPool>>() {
            Some(Initialized(pool)) => (
                pool.revision(),
                pool.eviction_generation(),
                pool.has_capacity(),
            ),
            _ => (0, 0, false),
        };
    #[cfg(feature = "raster")]
    let raster_mutation_gen = if has_raster {
        match world
            .resources
            .get::<Eventually<crate::raster::resource::RasterResources>>()
        {
            Some(Initialized(raster)) => raster.mutation_generation(),
            _ => 0,
        }
    } else {
        0
    };
    #[cfg(not(feature = "raster"))]
    let raster_mutation_gen = 0_u64;

    #[cfg(feature = "raster")]
    let raster_tiles_len = if has_raster {
        world
            .tiles
            .pyramid_of::<crate::raster::RasterLayersDataComponent>()
            .len()
    } else {
        0
    };
    #[cfg(not(feature = "raster"))]
    let raster_tiles_len = 0_usize;

    #[cfg(not(feature = "raster"))]
    let _ = has_raster;

    PatternEpochSnapshot {
        has_active_cross_fade,
        vector_pool_rev,
        vector_pool_eviction_gen,
        vector_pool_has_capacity,
        vector_arrival_epoch: crate::vector::arrival_epoch(world),
        vector_upload_stamp_epoch: crate::vector::upload_stamp_epoch(world),
        vector_tiles_len: world.tiles.pyramid_of::<VectorLayerBucketComponent>().len(),
        raster_tiles_len,
        raster_mutation_gen,
    }
}

fn try_reuse_memo(
    world: &mut World,
    view_region: &ViewRegion,
    raster_coverings: &[(RasterSourceId, Vec<WorldTileCoords>)],
    fast_key: Option<&FastPatternKey>,
    snapshot: PatternEpochSnapshot,
    (style_rev, zoom): (u64, Zoom),
) -> Result<Option<Option<PatternMemoKey>>, SystemError> {
    let tiles = &world.tiles;
    let Some((memo, pattern)) = world.resources.query_mut::<(
        &mut TileViewPatternMemo,
        &mut Eventually<WgpuTileViewPattern>,
    )>() else {
        return Ok(Some(compute_memo_key(
            view_region,
            raster_coverings,
            style_rev,
            snapshot,
            &|canonical| {
                tiles
                    .query::<&VectorLayerBucketComponent>(canonical)
                    .map(|buckets| (buckets.done, buckets.failed, buckets.uploaded_stamp))
            },
        )));
    };
    let Initialized(pattern) = pattern else {
        return Err(SystemError::Dependencies);
    };
    if memo.try_reuse_fast(pattern, fast_key, zoom) {
        return Ok(None);
    }
    let next_key = compute_memo_key(
        view_region,
        raster_coverings,
        style_rev,
        snapshot,
        &|canonical| {
            tiles
                .query::<&VectorLayerBucketComponent>(canonical)
                .map(|buckets| (buckets.done, buckets.failed, buckets.uploaded_stamp))
        },
    );
    if memo.try_reuse_full(pattern, next_key.as_ref(), fast_key.cloned(), zoom) {
        return Ok(None);
    }
    Ok(Some(next_key))
}

pub fn tile_view_pattern_system(
    MapContext {
        style,
        view_state,
        world,
        ..
    }: &mut MapContext,
) -> SystemResult {
    let level: ZoomLevel = view_state.zoom().zoom_level(DEFAULT_TILE_SIZE);
    let (view_region, raster_coverings) =
        drawn_covering(style, view_state, world, level).map_err(|error| {
            tracing::error!(%error, "unable to select tiles for rendering");
            SystemError::Setup
        })?;
    if let Some(view_region) = &view_region {
        let zoom = view_state.zoom();
        let style_rev = style.revision();
        let snapshot = capture_epoch_snapshot(world, !raster_coverings.is_empty());
        let tiles_arc: Option<Arc<[WorldTileCoords]>> =
            matching_drawn_tiles_arc(style, view_state, world, level);
        let fast_key = tiles_arc
            .as_ref()
            .and_then(|arc| compute_fast_memo_key(arc, &raster_coverings, style_rev, snapshot));
        let Some(next_key) = try_reuse_memo(
            world,
            view_region,
            &raster_coverings,
            fast_key.as_ref(),
            snapshot,
            (style_rev, zoom),
        )?
        else {
            return Ok(());
        };
        let mut view_tiles = world
            .resources
            .get_mut::<TileViewPatternMemo>()
            .map_or_else(Vec::new, TileViewPatternMemo::begin_rebuild);
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
        memo.finish_rebuild(view_tiles, next_key, fast_key, zoom);
    }

    Ok(())
}
