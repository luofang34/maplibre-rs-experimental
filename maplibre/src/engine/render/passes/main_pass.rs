//! Draws ordered style layers and the ground surface with each layer's own tile clipping masks.

use std::ops::Deref;

use wgpu::StoreOp;

pub(crate) use crate::render::render_commands::stencil_groups::{
    apply_layer_masks, unique_vector_stencils, StencilState, EMPTY_STENCIL_REFERENCE, STENCIL_MARK,
};
use crate::{
    render::{
        graph::{Node, RenderContext},
        render_commands::stencil_groups::{resolve_pass_layer_state, same_layer, PassLayerState},
        render_phase::{LayerItem, RenderPhase},
        resource::Texture,
        Eventually::Initialized,
        RenderResources,
    },
    tcs::world::World,
};

pub struct MainPassNode {}

impl MainPassNode {
    pub fn new() -> Self {
        Self {}
    }
}

impl Node for MainPassNode {
    fn writes_custom_scene_depth(&self) -> bool {
        false
    }

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
        let translucent_has_work = super::translucent_pass::has_work(world);
        let inline_translucent = render_context.inline_translucent_pass;
        let translucent_runs_separately = !inline_translucent && translucent_has_work;
        let downstream_loads_attachments =
            translucent_runs_separately || render_context.between_main_and_translucent_has_work;
        let keep_depth_stencil = downstream_loads_attachments
            || state.eye_depth_target.is_some()
            || matches!(state.depth_readout_texture, Initialized(_));
        let depth_stencil_store = if keep_depth_stencil {
            StoreOp::Store
        } else {
            StoreOp::Discard
        };
        let color = color_attachment(
            render_target.deref(),
            multisampling_texture.as_ref(),
            downstream_loads_attachments,
        );
        let pass = render_context
            .command_encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                multiview_mask: None,
                label: cfg!(debug_assertions).then_some("main_pass"),
                color_attachments: &[Some(color)],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_texture,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0),
                        store: depth_stencil_store,
                    }),
                    stencil_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(EMPTY_STENCIL_REFERENCE),
                        store: depth_stencil_store,
                    }),
                }),
                timestamp_writes: world
                    .resources
                    .get::<crate::render::gpu_timer::GpuTimerSlot>()
                    .and_then(|slot| slot.0.as_ref())
                    .and_then(crate::render::gpu_timer::GpuTimer::pass_writes),
                occlusion_query_set: None,
            });
        let frame_refs = crate::render::pass_refs::PassFrameRefs::from_world(world);
        let mut pass =
            crate::render::tracked_pass::TrackedRenderPass::new(pass).with_frame_refs(frame_refs);
        let inline_ctx = draw_layers(&mut pass, world);
        if inline_translucent && translucent_has_work {
            super::translucent_pass::draw_contents_inline(&mut pass, world, inline_ctx);
        }
        pass.finish(world);
    }
}

fn draw_layers<'w>(
    pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    world: &'w World,
) -> PassLayerState<'w> {
    let ground = world
        .resources
        .get::<crate::render::ground::FrameGround>()
        .and_then(|ground| ground.0);
    let mut ground_pending = ground.is_some();
    // Pass hooks at the beginning of main pass (e.g. starfield).
    if let Some(hooks) = world.resources.get::<crate::render::PassDrawHooks>() {
        for hook in hooks.hooks(crate::render::PassHookPoint::MainPassBegin) {
            if hook.has_work(world) {
                hook.draw(pass, world);
            }
        }
    }
    let layer_state = resolve_pass_layer_state(world);
    if let Some(layers) = world.resources.get::<RenderPhase<LayerItem>>() {
        let mut stencil_state = StencilState::Cleared;
        let all = layers.into_iter().as_slice();
        let drawn = all
            .len()
            .saturating_sub(layer_state.above.as_ref().map_or(0, |a| a.items.len()));
        for group in all.get(..drawn).unwrap_or(all).chunk_by(same_layer) {
            if let Some(masks) = layer_state.masks {
                apply_layer_masks(
                    pass,
                    world,
                    masks,
                    layer_state.unique_stencils,
                    &mut stencil_state,
                    group,
                );
            }
            if !ground_pending {
                for layer in group {
                    layer.draw_function.draw(pass, world, layer);
                }
            } else if let Some(surface) = ground {
                for layer in group {
                    // The ground preserves depth for screen-space layers while remaining above the
                    // background.
                    if ground_pending && layer.index > surface.draw_after_layer_index {
                        (surface.draw)(pass, world);
                        ground_pending = false;
                    }
                    layer.draw_function.draw(pass, world, layer);
                }
            }
        }
    }
    if let Some(surface) = ground.filter(|_| ground_pending) {
        (surface.draw)(pass, world);
    }
    layer_state
}

fn color_attachment<'a>(
    target: &'a wgpu::TextureView,
    multisampled: Option<&'a Texture>,
    keep_multisampled: bool,
) -> wgpu::RenderPassColorAttachment<'a> {
    let (view, resolve_target, store) = match multisampled {
        Some(texture) => (
            texture.deref(),
            Some(target),
            if keep_multisampled {
                StoreOp::Store
            } else {
                StoreOp::Discard
            },
        ),
        None => (target, None, StoreOp::Store),
    };
    wgpu::RenderPassColorAttachment {
        depth_slice: None,
        view,
        resolve_target,
        ops: wgpu::Operations {
            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
            store,
        },
    }
}
