//! Draws ordered style layers and the ground surface with each layer's own tile clipping masks.

use std::ops::Deref;

use wgpu::StoreOp;

use crate::{
    render::{
        graph::{Node, RenderContext},
        render_commands::{DrawMask, SetMaskPipeline},
        render_phase::{
            LayerItem, MaskGroup, RenderCommand, RenderCommandResult, RenderPhase, TileMaskItem,
            TranslucentItem,
        },
        resource::Texture,
        Eventually,
        Eventually::Initialized,
        MaskPipeline, RenderResources,
    },
    tcs::world::World,
};

// Valid tiles use `0..=123` (`(z % 31) * 4 + quadrant`). `0x7f` (`127`) stays outside all tile
// references while keeping bit 7 (`0x80`, `STENCIL_MARK`) clear so 3D extrusions rising above
// unmasked horizon/background pixels are not rejected by `extrusion_color_pipeline`.
pub(crate) const EMPTY_STENCIL_REFERENCE: u32 = 0x7f;

/// High stencil bit (`0x80`) marking pixels already drawn by the extrusion color pass.
/// Both tile references (`0..=123`) and `EMPTY_STENCIL_REFERENCE` (`0x7f`) keep this bit zero.
pub(crate) const STENCIL_MARK: u32 = 0x80;

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
                    view: &depth_texture.view,
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
        let base_vertex = world
            .resources
            .get::<crate::render::tracked_pass::DrawCapabilities>()
            .is_some_and(|cap| cap.base_vertex);
        let mut pass =
            crate::render::tracked_pass::TrackedRenderPass::new(pass).with_base_vertex(base_vertex);
        let inline_ctx = draw_layers(&mut pass, world);
        if inline_translucent && translucent_has_work {
            super::translucent_pass::draw_contents_inline(&mut pass, world, inline_ctx);
        }
        pass.finish(world);
    }
}

/// Active stencil contents left in the current render target.
#[derive(Clone, Copy, Default)]
pub(crate) enum StencilState<'w> {
    /// Target holds [`EMPTY_STENCIL_REFERENCE`] everywhere.
    #[default]
    Cleared,
    /// All `TileKind::Vector` masks are stamped coarse-to-fine and bit 7 (`STENCIL_MARK`) is zero.
    Vector,
    /// Masks belonging to this mutating layer group were stamped and possibly bitwise-inverted.
    Mutated(&'w [LayerItem]),
    /// Arbitrary stencil values may remain after a custom layer draw.
    DirtyAll,
}

pub(crate) fn unique_vector_stencils(masks: &[TileMaskItem]) -> bool {
    let mut seen = [0_u64; 2];
    masks
        .iter()
        .filter(|mask| mask.is_vector && !mask.generate_borders)
        .all(|mask| {
            let reference = usize::from(mask.source_shape.stencil_reference());
            let Some(word) = seen.get_mut(reference / 64) else {
                return false;
            };
            let bit = 1_u64 << (reference % 64);
            let fresh = *word & bit == 0;
            *word |= bit;
            fresh
        })
}

fn next_stencil_state<'w>(
    state: StencilState<'w>,
    unique_stencils: bool,
    group: &'w [LayerItem],
) -> StencilState<'w> {
    let Some(first) = group.first() else {
        return state;
    };
    match first.mask_group {
        MaskGroup::Unmasked => state,
        MaskGroup::Vector if unique_stencils => StencilState::Vector,
        MaskGroup::Vector | MaskGroup::Mutating => StencilState::Mutated(group),
        MaskGroup::Custom => StencilState::DirtyAll,
    }
}

/// The layers that draw after the first symbol layer, with the stencil state left before it.
///
/// Symbols draw in their own pass so they can read the depth the layers below them left; the
/// layers above the first symbol layer join that pass in order, so they cover symbols as the
/// style's order says. A frame with a ground surface draws everything in the main pass.
pub(super) struct AboveSymbols<'w> {
    /// Stencil state left at the end of the main pass.
    pub stencil_state: StencilState<'w>,
    /// Items of the layers that draw after the first symbol layer.
    pub items: &'w [LayerItem],
}

pub(super) struct PassLayerState<'w> {
    pub masks: Option<&'w [TileMaskItem]>,
    pub unique_stencils: bool,
    pub above: Option<AboveSymbols<'w>>,
}

pub(super) fn resolve_pass_layer_state(world: &World) -> PassLayerState<'_> {
    let masks = world
        .resources
        .get::<RenderPhase<TileMaskItem>>()
        .map(|phase| phase.into_iter().as_slice());
    let unique_stencils = masks.is_some_and(unique_vector_stencils);
    let above = above_symbols_with_masks(world, masks, unique_stencils);
    PassLayerState {
        masks,
        unique_stencils,
        above,
    }
}

fn above_symbols_with_masks<'w>(
    world: &'w World,
    masks: Option<&'w [TileMaskItem]>,
    unique_stencils: bool,
) -> Option<AboveSymbols<'w>> {
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
    let (below, above_items) = items.split_at_checked(split).unwrap_or((items, &[]));
    if above_items.is_empty() {
        return Some(AboveSymbols {
            stencil_state: StencilState::Cleared,
            items: &[],
        });
    }
    let masks_ready = masks.is_some()
        && matches!(
            world.resources.query::<(
                &Eventually<MaskPipeline>,
                &Eventually<crate::render::projection::ProjectionGpuResources>,
            )>(),
            Some((Initialized(_), Initialized(_)))
        );
    let mut stencil_state = StencilState::Cleared;
    if masks_ready {
        for group in below.chunk_by(same_layer) {
            stencil_state = next_stencil_state(stencil_state, unique_stencils, group);
        }
    }
    Some(AboveSymbols {
        stencil_state,
        items: above_items,
    })
}

pub(super) fn same_layer(left: &LayerItem, right: &LayerItem) -> bool {
    left.index == right.index && left.style_layer == right.style_layer
}

/// Draws one layer's items inside its tile masks, updating `stencil_state`.
pub(super) fn draw_group<'w>(
    pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    world: &'w World,
    masks: Option<&'w [TileMaskItem]>,
    unique_stencils: bool,
    stencil_state: &mut StencilState<'w>,
    group: &'w [LayerItem],
) {
    if let Some(masks) = masks {
        apply_layer_masks(pass, world, masks, unique_stencils, stencil_state, group);
    }
    for layer in group {
        layer.draw_function.draw(pass, world, layer);
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
        }
    }
    if let Some(surface) = ground.filter(|_| ground_pending) {
        (surface.draw)(pass, world);
    }
    layer_state
}

fn owns_mask(mask: &TileMaskItem, layers: &[LayerItem]) -> bool {
    layers
        .iter()
        .any(|layer| layer.source_shape.buffer_range() == mask.source_shape.buffer_range())
}

fn render_mask_list<'w, I: Iterator<Item = &'w TileMaskItem>>(
    (world, pattern, mesh_cache): (
        &'w World,
        &'w crate::render::tile_view_pattern::WgpuTileViewPattern,
        &'w crate::render::tile_mesh::GlobeTileMeshCache,
    ),
    pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    masks: I,
    fixed_reference: Option<u32>,
) {
    for mask in masks {
        let reference =
            fixed_reference.unwrap_or_else(|| u32::from(mask.source_shape.stencil_reference()));
        DrawMask::render_prepared(world, pattern, mesh_cache, mask, pass, reference);
    }
}

fn clear_stale_masks<'w>(
    ctx: (
        &'w World,
        &'w crate::render::tile_view_pattern::WgpuTileViewPattern,
        &'w crate::render::tile_mesh::GlobeTileMeshCache,
    ),
    pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    masks: &'w [TileMaskItem],
    state: StencilState<'w>,
) {
    match state {
        StencilState::Cleared => {}
        StencilState::Vector => render_mask_list(
            ctx,
            pass,
            masks.iter().filter(|m| m.is_vector),
            Some(EMPTY_STENCIL_REFERENCE),
        ),
        StencilState::Mutated(previous) => render_mask_list(
            ctx,
            pass,
            masks.iter().filter(|m| owns_mask(m, previous)),
            Some(EMPTY_STENCIL_REFERENCE),
        ),
        StencilState::DirtyAll => {
            render_mask_list(ctx, pass, masks.iter(), Some(EMPTY_STENCIL_REFERENCE))
        }
    }
}

pub(crate) fn apply_layer_masks<'w>(
    pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    world: &'w World,
    masks: &'w [TileMaskItem],
    unique_stencils: bool,
    state: &mut StencilState<'w>,
    current: &'w [LayerItem],
) {
    let Some(first) = current.first() else {
        return;
    };
    match first.mask_group {
        MaskGroup::Unmasked => return,
        MaskGroup::Custom => {
            *state = StencilState::DirtyAll;
            return;
        }
        MaskGroup::Vector if matches!(*state, StencilState::Vector) => return,
        MaskGroup::Vector | MaskGroup::Mutating => {}
    }

    let Some(first_mask) = masks.first() else {
        *state = next_stencil_state(*state, unique_stencils, current);
        return;
    };
    if matches!(
        SetMaskPipeline::render(world, first_mask, pass),
        RenderCommandResult::Failure
    ) {
        return;
    }
    let Some((Initialized(pattern), mesh_cache)) = world.resources.query::<(
        &Eventually<crate::render::tile_view_pattern::WgpuTileViewPattern>,
        &crate::render::tile_mesh::GlobeTileMeshCache,
    )>() else {
        return;
    };

    let ctx = (world, pattern, mesh_cache);
    clear_stale_masks(ctx, pass, masks, *state);
    match first.mask_group {
        MaskGroup::Vector if unique_stencils => {
            render_mask_list(ctx, pass, masks.iter().filter(|m| m.is_vector), None);
            *state = StencilState::Vector;
        }
        MaskGroup::Vector | MaskGroup::Mutating => {
            render_mask_list(
                ctx,
                pass,
                masks.iter().filter(|m| owns_mask(m, current)),
                None,
            );
            *state = StencilState::Mutated(current);
        }
        MaskGroup::Unmasked | MaskGroup::Custom => {}
    }
}

fn color_attachment<'a>(
    target: &'a wgpu::TextureView,
    multisampled: Option<&'a Texture>,
    keep_multisampled: bool,
) -> wgpu::RenderPassColorAttachment<'a> {
    let (view, resolve_target, store) = match multisampled {
        Some(texture) => (
            texture.view.deref(),
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
