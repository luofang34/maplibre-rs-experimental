//! Synchronizes GPU symbol fade uniforms and prunes finished symbol draw items.

use rustc_hash::FxHashMap;

use super::{signatures::has_symbol_depth_occluders, temporal::PlacementHistory, upload_diff};
use crate::{
    coords::WorldTileCoords,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        render_phase::{RenderPhase, TranslucentItem},
        Renderer,
    },
    sdf::{depth::SymbolDepth, SymbolBufferPool},
    style::StyleLayerId,
    tcs::world::World,
};

pub(super) fn sync_symbol_depth_fade(
    world: &mut World,
    renderer: &Renderer,
    history: &PlacementHistory,
    pitch: f64,
) {
    let time_ms = history.effective_shader_time_ms();
    let duration_ms = history.fade_duration_ms();
    let skip_depth_occlusion = !has_symbol_depth_occluders(world, &renderer.render_graph, pitch);
    if let Some(Initialized(depth)) = world.resources.get_mut::<Eventually<SymbolDepth>>() {
        depth.write_fade_time(
            &renderer.device,
            &renderer.queue,
            time_ms,
            duration_ms,
            skip_depth_occlusion,
        );
    }
}

pub(super) fn prune_inactive_symbol_draws(
    world: &mut World,
    history: &PlacementHistory,
    uploaded: &FxHashMap<(WorldTileCoords, StyleLayerId), upload_diff::CachedBucketMetadata>,
) {
    let has_symbol_items = world
        .resources
        .get::<RenderPhase<TranslucentItem>>()
        .is_some_and(|phase| {
            phase
                .items()
                .iter()
                .any(|item| item.style_layer != StyleLayerId::ATMOSPHERE && item.index != u32::MAX)
        });
    if !has_symbol_items {
        return;
    }
    let now_ms = history.effective_shader_time_ms();
    let fade_duration_ms = history.fade_duration_ms();
    let Some((phase, Initialized(pool))) = world.resources.query_mut::<(
        &mut RenderPhase<TranslucentItem>,
        &Eventually<SymbolBufferPool>,
    )>() else {
        return;
    };
    phase.retain(|item| {
        if item.style_layer == StyleLayerId::ATMOSPHERE || item.index == u32::MAX {
            return true;
        }
        let Some(entry) =
            pool.index()
                .resolve_entry(item.tile.coords, item.style_layer, item.entry_slot)
        else {
            return false;
        };
        uploaded
            .get(&(item.tile.coords, item.style_layer))
            .is_some_and(|cached| {
                cached.allocation_id == entry.allocation_id()
                    && cached.is_active_at(now_ms, fade_duration_ms)
            })
    });
}
