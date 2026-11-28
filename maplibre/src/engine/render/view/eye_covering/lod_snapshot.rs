//! In-place frame LOD history snapshot tracking without per-frame resource reallocations.

use std::sync::Arc;

use rustc_hash::FxHashMap;

use super::{EyeInFrame, FrameLodHistory, SharedCovering};
use crate::{
    coords::WorldTileCoords, io::tile_sources::RasterSourceId, projection::lod_history::LodHistory,
    tcs::world::World,
};

#[derive(Default)]
struct LodHistoryStamp {
    initialized: bool,
    view: Option<Arc<[WorldTileCoords]>>,
}

fn sync_raster_history(
    dst: &mut FxHashMap<RasterSourceId, LodHistory>,
    src: &[(RasterSourceId, Vec<WorldTileCoords>)],
) {
    if src.is_empty() {
        if !dst.is_empty() {
            dst.clear();
        }
        return;
    }
    dst.retain(|id, _| src.iter().any(|(src_id, _)| src_id == id));
    for (id, tiles) in src {
        if let Some(entry) = dst.get_mut(id) {
            entry.rebuild(tiles);
        } else {
            dst.insert(id.clone(), LodHistory::new(tiles));
        }
    }
}

fn next_terrain_history(world: &World) -> Option<LodHistory> {
    let tiles = world
        .resources
        .get::<crate::render::projection::TerrainElevationBridge>()
        .map_or(&[][..], |bridge| (bridge.drawn_surface_tiles)(world));
    if tiles.is_empty() {
        world
            .resources
            .get::<FrameLodHistory>()
            .is_some_and(|history| !history.terrain.is_empty())
            .then(LodHistory::default)
    } else {
        Some(LodHistory::new(tiles))
    }
}

/// Snapshots the frame's drawn tiles into [`FrameLodHistory`], mutating collections in-place
/// and skipping view rebuilds when [`SharedCovering`] still points to the same `Arc` slice.
pub(crate) fn snapshot_lod_history(world: &mut World) {
    if EyeInFrame::reuses_content(world) {
        return;
    }
    let next_terrain = next_terrain_history(world);
    if !world.resources.exists::<FrameLodHistory>() {
        world.resources.insert(FrameLodHistory::default());
    }
    if !world.resources.exists::<LodHistoryStamp>() {
        world.resources.insert(LodHistoryStamp::default());
    }
    if world.resources.exists::<SharedCovering>() {
        let Some((shared, history, stamp)) =
            world
                .resources
                .query_mut::<(&SharedCovering, &mut FrameLodHistory, &mut LodHistoryStamp)>()
        else {
            return;
        };
        if let Some(terrain) = next_terrain {
            history.terrain = terrain;
        }
        let view_arc = shared.tiles.as_ref().map(|(tiles, _)| tiles);
        let view_same = stamp.initialized
            && match (stamp.view.as_ref(), view_arc) {
                (None, None) => true,
                (Some(prev), Some(curr)) => Arc::ptr_eq(prev, curr),
                _ => false,
            };
        if !view_same {
            history.view.rebuild(view_arc.map_or(&[], |tiles| &**tiles));
            stamp.view = view_arc.cloned();
        }
        stamp.initialized = true;
        sync_raster_history(&mut history.raster, &shared.raster);
    } else {
        let Some((history, stamp)) = world
            .resources
            .query_mut::<(&mut FrameLodHistory, &mut LodHistoryStamp)>()
        else {
            return;
        };
        if let Some(terrain) = next_terrain {
            history.terrain = terrain;
        }
        if !stamp.initialized || stamp.view.is_some() {
            history.view.rebuild(&[]);
            stamp.view = None;
        }
        stamp.initialized = true;
        if !history.raster.is_empty() {
            history.raster.clear();
        }
    }
}
