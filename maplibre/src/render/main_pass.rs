//! Draws ordered style layers and the ground surface with each layer's own tile clipping masks.

use std::ops::Deref;

use wgpu::StoreOp;

use crate::{
    render::{
        graph::{Node, RenderContext},
        render_commands::{DrawMask, SetMaskPipeline},
        render_phase::{
            LayerItem, RenderCommand, RenderCommandResult, RenderPhase, TileMaskItem,
            TranslucentItem,
        },
        resource::Texture,
        Eventually::Initialized,
        RenderResources,
    },
    tcs::world::World,
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
    fn run(&self, render_context: &mut RenderContext, state: &RenderResources, world: &World) {
        let Initialized(render_target) = &state.render_target else {
            return;
        };
        let Initialized(multisampling_texture) = &state.multisampling_texture else {
            return;
        };
        let Initialized(depth_texture) = &state.depth_texture else {
            return;
        };
        let color = color_attachment(render_target.deref(), multisampling_texture.as_ref());
        let pass = render_context
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
                timestamp_writes: world
                    .resources
                    .get::<crate::render::gpu_timer::GpuTimerSlot>()
                    .and_then(|slot| slot.0.as_ref())
                    .and_then(crate::render::gpu_timer::GpuTimer::pass_writes),
                occlusion_query_set: None,
            });
        let mut pass = crate::render::tracked_pass::TrackedRenderPass::new(pass);
        draw_layers(&mut pass, world);
        pass.finish(world);
    }
}

/// The layers that draw after the first symbol layer, with the last group drawn before it.
///
/// Symbols draw in their own pass so they can read the depth the layers below them left; the
/// layers above the first symbol layer join that pass in order, so they cover symbols as the
/// style's order says. A frame with a ground surface draws everything in the main pass.
pub(super) struct AboveSymbols<'w> {
    /// The last group of layer items the main pass draws.
    pub previous: &'w [LayerItem],
    /// Items of the layers that draw after the first symbol layer.
    pub items: &'w [LayerItem],
}

pub(super) fn above_symbols(world: &World) -> Option<AboveSymbols<'_>> {
    if world
        .resources
        .get::<crate::render::ground::FrameGround>()
        .is_some_and(|ground| ground.0.is_some())
    {
        return None;
    }
    let cutoff = world
        .resources
        .get::<RenderPhase<TranslucentItem>>()?
        .into_iter()
        .map(|item| item.index)
        .filter(|index| *index != u32::MAX)
        .min()?;
    let layers = world.resources.get::<RenderPhase<LayerItem>>()?;
    let items = layers.into_iter().as_slice();
    let split = items.partition_point(|item| item.index < cutoff);
    let below = &items[..split];
    let previous = below.chunk_by(same_layer).last().unwrap_or(&[][..]);
    Some(AboveSymbols {
        previous,
        items: &items[split..],
    })
}

pub(super) fn same_layer(left: &LayerItem, right: &LayerItem) -> bool {
    left.index == right.index && left.style_layer == right.style_layer
}

/// Draws one layer's items inside its tile masks, replacing the masks of `previous`.
pub(super) fn draw_group<'w>(
    pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    world: &'w World,
    previous: &[LayerItem],
    group: &'w [LayerItem],
) {
    set_layer_masks(pass, world, previous, group);
    for layer in group {
        layer.draw_function.draw(pass, world, layer);
    }
}

fn draw_layers<'w>(
    pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    world: &'w World,
) {
    let ground = world
        .resources
        .get::<crate::render::ground::FrameGround>()
        .and_then(|ground| ground.0);
    let mut ground_pending = ground.is_some();
    // Pass hooks at the beginning of main pass (e.g. starfield).
    let pass_hooks = world
        .resources
        .get::<crate::render::PassDrawHooks>()
        .map(|hooks| hooks.get_hooks(crate::render::PassHookPoint::MainPassBegin));
    if let Some(hooks) = pass_hooks {
        for hook in hooks {
            hook.draw(pass, world);
        }
    }
    let above = above_symbols(world);
    if let Some(layers) = world.resources.get::<RenderPhase<LayerItem>>() {
        let mut previous = &[][..];
        let all = layers.into_iter().as_slice();
        let drawn = all.len() - above.as_ref().map_or(0, |above| above.items.len());
        for group in all[..drawn].chunk_by(same_layer) {
            set_layer_masks(pass, world, previous, group);
            for layer in group {
                // The ground preserves depth for screen-space layers while remaining above the
                // background.
                if let Some(surface) = ground.filter(|surface| {
                    ground_pending && layer.index > surface.draw_after_layer_index
                }) {
                    (surface.draw)(pass, world);
                    ground_pending = false;
                }
                layer.draw_function.draw(pass, world, layer);
            }
            previous = group;
        }
    }
    if let Some(surface) = ground.filter(|_| ground_pending) {
        (surface.draw)(pass, world);
    }
}

fn set_layer_masks<'w>(
    pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
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
