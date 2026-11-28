//! Draws a stable symbol covering independently of terrain texture readiness.
use crate::{
    context::MapContext,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        render_phase::{DrawState, RenderPhase, TranslucentItem},
        shaders::ShaderTileMetadata,
        tile_view_pattern::ShapeDrawData,
    },
    schedule::system::{SystemError, SystemResult},
    sdf::{covering::SymbolCovering, render_commands::DrawSymbols, SymbolBufferPool},
    tcs::tiles::Tile,
};

pub fn queue_system(
    MapContext {
        world,
        style,
        view_state,
        renderer,
        ..
    }: &mut MapContext,
) -> SystemResult {
    if renderer.settings.symbol_pools.is_empty() {
        return Ok(());
    }
    if !crate::sdf::style_has_symbol_layers(style) && crate::sdf::symbol_pool_empty(world) {
        return Ok(());
    }
    crate::sdf::covering::update(world, style, view_state, &renderer.device, &renderer.queue);
    if world
        .resources
        .get::<SymbolCovering>()
        .is_none_or(|covering| covering.tiles.is_empty())
    {
        return if world
            .resources
            .get::<RenderPhase<TranslucentItem>>()
            .is_some()
            && matches!(
                world.resources.get::<Eventually<SymbolBufferPool>>(),
                Some(Initialized(_))
            ) {
            Ok(())
        } else {
            Err(SystemError::Dependencies)
        };
    }
    let Some((covering, phase, Initialized(pool))) = world.resources.query_mut::<(
        &mut SymbolCovering,
        &mut RenderPhase<TranslucentItem>,
        &Eventually<SymbolBufferPool>,
    )>() else {
        return Err(SystemError::Dependencies);
    };
    let stride = size_of::<ShaderTileMetadata>() as u64;
    let SymbolCovering {
        tiles,
        tiles_version,
        visible_hash,
        phase_key,
        queue_scratch,
        ..
    } = covering;
    let next_phase_key = (
        *tiles_version,
        style.revision(),
        *visible_hash,
        pool.revision(),
        pool.eviction_generation(),
    );
    if *phase_key == Some(next_phase_key) {
        phase.extend_from_slice(queue_scratch);
        return Ok(());
    }
    queue_scratch.clear();
    let style_zoom = view_state.style_zoom().value();
    for (index, coords) in tiles.iter().enumerate() {
        let start = index as u64 * stride;
        let shape = ShapeDrawData::from_buffer_range(*coords, start..start + stride);
        for (slot_idx, layer) in pool
            .as_ref()
            .get_layers(*coords)
            .into_iter()
            .flatten()
            .enumerate()
        {
            if !layer.style_layer.is_visible_at(style_zoom) {
                continue;
            }
            queue_scratch.push(TranslucentItem {
                draw_function: &DrawState::<TranslucentItem, DrawSymbols>::INSTANCE,
                index: layer.style_layer.index,
                style_layer: layer.style_layer_id,
                entry_slot: u16::try_from(slot_idx)
                    .unwrap_or(crate::render::render_phase::UNSET_ENTRY_SLOT),
                tile: Tile { coords: *coords },
                source_shape: shape,
            });
        }
    }
    if !queue_scratch.is_sorted_by_key(|item| item.index) {
        queue_scratch.sort_by_key(|item| item.index);
    }
    *phase_key = Some(next_phase_key);
    phase.extend_from_slice(queue_scratch);
    Ok(())
}
