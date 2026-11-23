use std::ops::Deref;

use wgpu::StoreOp;

use crate::{
    render::{
        graph::{Node, RenderContext},
        render_phase::{RenderPhase, TranslucentItem},
        Eventually::Initialized,
        RenderResources,
    },
    tcs::world::World,
};

pub struct TranslucentPassNode {}

impl TranslucentPassNode {
    pub fn new() -> Self {
        Self {}
    }
}

pub(crate) fn has_work(world: &World) -> bool {
    if world
        .resources
        .get::<RenderPhase<TranslucentItem>>()
        .is_some_and(|items| items.size() > 0)
    {
        return true;
    }
    world
        .resources
        .get::<crate::render::PassDrawHooks>()
        .is_some_and(|hooks| {
            hooks.has_active_work(crate::render::PassHookPoint::TranslucentPassBegin, world)
                || hooks.has_active_work(crate::render::PassHookPoint::TranslucentPassEnd, world)
        })
}

pub(crate) fn needs_symbol_depth_copy(world: &World) -> bool {
    world
        .resources
        .get::<RenderPhase<TranslucentItem>>()
        .is_some_and(|items| items.into_iter().any(|item| item.index != u32::MAX))
        && matches!(
            world
                .resources
                .get::<crate::render::eventually::Eventually<crate::sdf::depth::SymbolDepth>>(),
            Some(Initialized(symbol_depth)) if !symbol_depth.skip_depth_copy
        )
}

fn run_translucent_hooks<'w>(
    render_pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    world: &'w World,
    point: crate::render::PassHookPoint,
) {
    if let Some(hooks) = world.resources.get::<crate::render::PassDrawHooks>() {
        for hook in hooks.hooks(point) {
            if hook.has_work(world) {
                hook.draw(render_pass, world);
            }
        }
    }
}

pub(crate) fn draw_contents_inline<'w>(
    render_pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    world: &'w World,
    layer_state: super::main_pass::PassLayerState<'w>,
) {
    run_translucent_hooks(
        render_pass,
        world,
        crate::render::PassHookPoint::TranslucentPassBegin,
    );

    if let Some(mask_items) = world.resources.get::<RenderPhase<TranslucentItem>>() {
        log::trace!(
            "RenderPhase<TranslucentItem>::size() = {}",
            mask_items.size()
        );
        let masks = layer_state.masks;
        let unique_stencils = layer_state.unique_stencils;
        let above = layer_state.above;
        let mut stencil_state = above
            .as_ref()
            .map_or(super::main_pass::StencilState::Cleared, |above| {
                above.stencil_state
            });
        let mut groups = above
            .as_ref()
            .map_or(&[][..], |above| above.items)
            .chunk_by(super::main_pass::same_layer)
            .peekable();
        let all_translucent = mask_items.into_iter().as_slice();
        for chunk in all_translucent.chunk_by(|a, b| {
            a.index == b.index
                && a.style_layer == b.style_layer
                && a.entry_slot != super::render_phase::UNSET_ENTRY_SLOT
                && b.entry_slot != super::render_phase::UNSET_ENTRY_SLOT
        }) {
            let Some(first_item) = chunk.first() else {
                continue;
            };
            while let Some(group) = groups.next_if(|group| {
                group
                    .first()
                    .is_some_and(|first| first.index < first_item.index)
            }) {
                super::main_pass::draw_group(
                    render_pass,
                    world,
                    masks,
                    unique_stencils,
                    &mut stencil_state,
                    group,
                );
            }
            if !crate::sdf::render_commands::try_draw_symbol_group(world, chunk, render_pass) {
                for item in chunk {
                    item.draw_function.draw(render_pass, world, item);
                }
            }
        }
        for group in groups {
            super::main_pass::draw_group(
                render_pass,
                world,
                masks,
                unique_stencils,
                &mut stencil_state,
                group,
            );
        }
    }

    run_translucent_hooks(
        render_pass,
        world,
        crate::render::PassHookPoint::TranslucentPassEnd,
    );
}

pub(crate) fn draw_contents<'w>(
    render_pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    world: &'w World,
) {
    let layer_state = super::main_pass::resolve_pass_layer_state(world);
    draw_contents_inline(render_pass, world, layer_state);
}

fn copy_symbol_depth_if_needed(
    render_context: &mut RenderContext,
    depth_texture: &wgpu::Texture,
    world: &World,
) {
    if !needs_symbol_depth_copy(world) {
        return;
    }
    if let Some(Initialized(symbol_depth)) = world
        .resources
        .get::<crate::render::eventually::Eventually<crate::sdf::depth::SymbolDepth>>()
    {
        if let Some(copy) = &symbol_depth.copy {
            copy.copy(
                render_context.device,
                &mut render_context.command_encoder,
                depth_texture,
                &symbol_depth.view,
            );
        }
    }
}

fn translucent_color_attachment<'a>(
    render_target: &'a crate::render::resource::TextureView,
    multisampling_texture: &'a Option<crate::render::resource::Texture>,
) -> wgpu::RenderPassColorAttachment<'a> {
    if let Some(texture) = multisampling_texture {
        wgpu::RenderPassColorAttachment {
            depth_slice: None,
            view: &texture.view,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Load,
                // Resolved into `render_target`; discarding the transient MSAA surface avoids
                // writing multisampled tile memory back to DRAM on mobile TBDR GPUs.
                store: StoreOp::Discard,
            },
            resolve_target: Some(render_target.deref()),
        }
    } else {
        wgpu::RenderPassColorAttachment {
            depth_slice: None,
            view: render_target.deref(),
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: StoreOp::Store,
            },
            resolve_target: None,
        }
    }
}

fn translucent_depth_attachment(
    depth_view: &wgpu::TextureView,
    needs_depth_after_pass: bool,
) -> wgpu::RenderPassDepthStencilAttachment<'_> {
    let store = if needs_depth_after_pass {
        StoreOp::Store
    } else {
        StoreOp::Discard
    };
    wgpu::RenderPassDepthStencilAttachment {
        view: depth_view,
        depth_ops: Some(wgpu::Operations {
            load: wgpu::LoadOp::Load,
            store,
        }),
        stencil_ops: Some(wgpu::Operations {
            load: wgpu::LoadOp::Load,
            store,
        }),
    }
}

impl Node for TranslucentPassNode {
    fn writes_custom_scene_depth(&self) -> bool {
        false
    }

    fn has_active_work(&self, _state: &RenderResources, world: &World) -> bool {
        has_work(world)
    }

    fn run(&self, render_context: &mut RenderContext, state: &RenderResources, world: &World) {
        if render_context.inline_translucent_pass || !has_work(world) {
            return;
        }
        let (
            Initialized(render_target),
            Initialized(multisampling_texture),
            Initialized(depth_texture),
        ) = (
            &state.render_target,
            &state.multisampling_texture,
            &state.depth_texture,
        )
        else {
            return;
        };
        copy_symbol_depth_if_needed(render_context, &depth_texture.texture, world);
        let color_attachment = translucent_color_attachment(render_target, multisampling_texture);
        let needs_depth_after_pass = state.eye_depth_target.is_some()
            || matches!(state.depth_readout_texture, Initialized(_));
        let depth_stencil_attachment =
            translucent_depth_attachment(&depth_texture.view, needs_depth_after_pass);
        let render_pass =
            render_context
                .command_encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    multiview_mask: None,
                    label: cfg!(debug_assertions).then_some("translucent_pass"),
                    color_attachments: &[Some(color_attachment)],
                    depth_stencil_attachment: Some(depth_stencil_attachment),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
        let frame_refs = crate::render::pass_refs::PassFrameRefs::from_world(world);
        let mut render_pass = crate::render::tracked_pass::TrackedRenderPass::new(render_pass)
            .with_frame_refs(frame_refs);
        draw_contents(&mut render_pass, world);
        render_pass.finish(world);
    }
}
