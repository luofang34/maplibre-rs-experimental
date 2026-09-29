//! Draws ordered style layers and terrain with each layer's own tile clipping masks.

use std::ops::Deref;

use wgpu::StoreOp;

use crate::{
    render::{
        draw_graph,
        graph::{Node, NodeRunError, RenderContext, RenderGraphContext, SlotInfo},
        render_commands::{DrawMask, SetMaskPipeline},
        render_phase::{LayerItem, RenderCommand, RenderCommandResult, RenderPhase, TileMaskItem},
        resource::Texture,
        Eventually::Initialized,
        RenderResources,
    },
    tcs::world::World,
    terrain::{draw_terrain, TerrainFrame},
};

// Valid tiles use four references per zoom; this value is outside that range, including z0.
const EMPTY_STENCIL_REFERENCE: u32 = crate::coords::MAX_ZOOM as u32 * 4;

pub struct MainPassNode {}

impl MainPassNode {
    pub fn new() -> Self {
        Self {}
    }
}

impl Node for MainPassNode {
    fn input(&self) -> Vec<SlotInfo> {
        vec![]
    }

    fn update(&mut self, _state: &mut RenderResources) {}

    fn run(
        &self,
        _graph: &mut RenderGraphContext,
        render_context: &mut RenderContext,
        state: &RenderResources,
        world: &World,
    ) -> Result<(), NodeRunError> {
        let Initialized(render_target) = &state.render_target else {
            return Ok(());
        };
        let Initialized(multisampling_texture) = &state.multisampling_texture else {
            return Ok(());
        };
        let Initialized(depth_texture) = &state.depth_texture else {
            return Ok(());
        };
        let color = color_attachment(render_target.deref(), multisampling_texture.as_ref());
        let mut pass =
            render_context
                .command_encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    multiview_mask: None,
                    label: Some("main_pass"),
                    color_attachments: &[Some(color)],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth_texture.view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(0.0),
                            store: StoreOp::Store,
                        }),
                        stencil_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(EMPTY_STENCIL_REFERENCE),
                            store: StoreOp::Store,
                        }),
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
        draw_layers(&mut pass, world);
        Ok(())
    }
}

fn draw_layers<'w>(pass: &mut wgpu::RenderPass<'w>, world: &'w World) {
    let terrain = world
        .resources
        .get::<TerrainFrame>()
        .filter(|frame| frame.active)
        .copied();
    let mut terrain_pending = terrain.is_some();
    if let Some(layers) = world.resources.get::<RenderPhase<LayerItem>>() {
        let mut previous = &[][..];
        for group in layers.into_iter().as_slice().chunk_by(|left, right| {
            left.index == right.index && left.style_layer == right.style_layer
        }) {
            set_layer_masks(pass, world, previous, group);
            for layer in group {
                // Terrain preserves depth for screen-space layers while remaining above the background.
                if terrain_pending
                    && terrain.is_some_and(|frame| layer.index > frame.draw_after_layer_index)
                {
                    draw_terrain(pass, world);
                    terrain_pending = false;
                }
                layer.draw_function.draw(pass, world, layer);
            }
            previous = group;
        }
    }
    if terrain_pending {
        draw_terrain(pass, world);
    }
}

fn set_layer_masks<'w>(
    pass: &mut wgpu::RenderPass<'w>,
    world: &'w World,
    previous: &[LayerItem],
    current: &[LayerItem],
) {
    let Some(masks) = world.resources.get::<RenderPhase<TileMaskItem>>() else {
        return;
    };
    let owns_mask = |mask: &TileMaskItem, layers: &[LayerItem]| {
        layers
            .iter()
            .any(|layer| layer.source_shape.buffer_range() == mask.source_shape.buffer_range())
    };
    // A new source pyramid must neither inherit old clipping nor admit buffered geometry.
    for mask in masks {
        if owns_mask(mask, previous)
            && matches!(
                SetMaskPipeline::render(world, mask, pass),
                RenderCommandResult::Success
            )
        {
            DrawMask::render_with_reference(world, mask, pass, EMPTY_STENCIL_REFERENCE);
        }
    }
    for mask in masks {
        if owns_mask(mask, current) {
            mask.draw_function.draw(pass, world, mask);
        }
    }
}

fn color_attachment<'a>(
    target: &'a wgpu::TextureView,
    multisampled: Option<&'a Texture>,
) -> wgpu::RenderPassColorAttachment<'a> {
    let (view, resolve_target) = match multisampled {
        Some(texture) => (texture.view.deref(), Some(target)),
        None => (target, None),
    };
    wgpu::RenderPassColorAttachment {
        depth_slice: None,
        view,
        resolve_target,
        ops: wgpu::Operations {
            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
            store: StoreOp::Store,
        },
    }
}

pub struct MainPassDriverNode;

impl Node for MainPassDriverNode {
    fn run(
        &self,
        graph: &mut RenderGraphContext,
        _render_context: &mut RenderContext,
        _resources: &RenderResources,
        _world: &World,
    ) -> Result<(), NodeRunError> {
        graph.run_sub_graph(draw_graph::NAME, vec![])?;
        Ok(())
    }
}
