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
    super::covering::update(world, style, view_state, &renderer.device, &renderer.queue);
    let Some((covering, phase, Initialized(pool))) = world.resources.query_mut::<(
        &SymbolCovering,
        &mut RenderPhase<TranslucentItem>,
        &Eventually<SymbolBufferPool>,
    )>() else {
        return Err(SystemError::Dependencies);
    };
    let stride = size_of::<ShaderTileMetadata>() as u64;
    for (index, coords) in covering.tiles.iter().enumerate() {
        let start = index as u64 * stride;
        let shape = TileShape::with_buffer_range(*coords, view_state.zoom(), start..start + stride);
        for layer in pool.index().get_layers(*coords).into_iter().flatten() {
            if !layer
                .style_layer
                .is_visible_at(view_state.style_zoom().value())
            {
                continue;
            }
            phase.add(TranslucentItem {
                draw_function: Box::new(DrawState::<TranslucentItem, DrawSymbols>::new()),
                index: layer.style_layer.index,
                style_layer: layer.style_layer.id.clone(),
                tile: Tile { coords: *coords },
                source_shape: shape.clone(),
            });
        }
    }
    Ok(())
}
