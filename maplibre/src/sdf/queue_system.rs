//! Draws a stable symbol covering independently of terrain texture readiness.
use crate::{
    context::MapContext,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        render_phase::{DrawState, RenderPhase, TranslucentItem},
        shaders::ShaderTileMetadata,
        tile_view_pattern::TileShape,
    },
    sdf::{covering::SymbolCovering, render_commands::DrawSymbols, SymbolBufferPool},
    tcs::{
        system::{SystemError, SystemResult},
        tiles::Tile,
    },
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
    if !super::style_has_symbol_layers(style) && super::symbol_pool_empty(world) {
        return Ok(());
    }
    super::covering::update(world, style, view_state, &renderer.device, &renderer.queue);
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
        queue_scratch,
        ..
    } = covering;
    queue_scratch.clear();
    let style_zoom = view_state.style_zoom().value();
    let zoom = view_state.zoom();
    for (index, coords) in tiles.iter().enumerate() {
        let start = index as u64 * stride;
        let shape = TileShape::with_buffer_range(*coords, zoom, start..start + stride).draw_data();
        for (slot_idx, layer) in pool
            .index()
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
    phase.extend_from_slice(queue_scratch);
    Ok(())
}
