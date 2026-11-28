//! Stencil state tracking and mask batching shared across the main and translucent render passes.

use super::{DrawMask, SetMaskPipeline};
use crate::{
    render::{
        eventually::{Eventually, Eventually::Initialized},
        pass_refs::PassFrameRefs,
        projection::ProjectionGpuResources,
        render_phase::{
            LayerDrawKind, LayerItem, MaskGroup, RenderCommand, RenderCommandResult, RenderPhase,
            TileMaskItem, TranslucentItem,
        },
        tile_mesh::GlobeTileMeshCache,
        tile_view_pattern::WgpuTileViewPattern,
        tracked_pass::TrackedRenderPass,
        MaskPipeline,
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
pub(crate) struct AboveSymbols<'w> {
    /// Stencil state left at the end of the main pass.
    pub stencil_state: StencilState<'w>,
    /// Items of the layers that draw after the first symbol layer.
    pub items: &'w [LayerItem],
}

pub(crate) struct PassLayerState<'w> {
    pub masks: Option<&'w [TileMaskItem]>,
    pub unique_stencils: bool,
    pub above: Option<AboveSymbols<'w>>,
}

pub(crate) fn resolve_pass_layer_state(world: &World) -> PassLayerState<'_> {
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
                &Eventually<ProjectionGpuResources>,
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

pub(crate) fn same_layer(left: &LayerItem, right: &LayerItem) -> bool {
    left.index == right.index && left.style_layer == right.style_layer
}

/// Computes the union of `PassFrameRefs::DOMAIN_*` bits required to draw `items`.
pub(crate) fn layer_items_ref_domains(items: &[LayerItem], mut domains: u8) -> u8 {
    let full =
        PassFrameRefs::DOMAIN_VECTOR | PassFrameRefs::DOMAIN_RASTER | PassFrameRefs::DOMAIN_TERRAIN;
    for item in items {
        let need = match item.draw_kind {
            LayerDrawKind::VectorLine => {
                PassFrameRefs::DOMAIN_VECTOR | PassFrameRefs::DOMAIN_TERRAIN
            }
            LayerDrawKind::VectorFill
            | LayerDrawKind::VectorCircle
            | LayerDrawKind::VectorPattern
            | LayerDrawKind::ExtrusionDepth
            | LayerDrawKind::ExtrusionColor
            | LayerDrawKind::ExtrusionPatternColor
            | LayerDrawKind::ExtrusionTranslucentBack
            | LayerDrawKind::ExtrusionTranslucentFront => PassFrameRefs::DOMAIN_VECTOR,
            LayerDrawKind::Raster => PassFrameRefs::DOMAIN_RASTER,
            LayerDrawKind::Other => PassFrameRefs::DOMAIN_DRAPE,
        };
        domains |= need;
        if (domains & full) == full {
            break;
        }
    }
    domains
}

/// Draws one layer's items inside its tile masks, updating `stencil_state`.
pub(crate) fn draw_group<'w>(
    pass: &mut TrackedRenderPass<'w>,
    world: &'w World,
    layer_hooks: Option<crate::render::graph::LayerPassHooks>,
    (masks, unique_stencils): (Option<&'w [TileMaskItem]>, bool),
    stencil_state: &mut StencilState<'w>,
    group: &'w [LayerItem],
) {
    if let Some(masks) = masks {
        apply_layer_masks(pass, world, masks, unique_stencils, stencil_state, group);
    }
    if !layer_hooks.is_some_and(|hooks| hooks.dispatch_group(world, group, pass)) {
        for layer in group {
            layer.draw_function.draw(pass, world, layer);
        }
    }
}

fn owns_mask(mask: &TileMaskItem, layers: &[LayerItem]) -> bool {
    layers
        .iter()
        .any(|layer| layer.source_shape.buffer_range() == mask.source_shape.buffer_range())
}

fn render_mask_list<'w, I: Iterator<Item = &'w TileMaskItem>>(
    (world, pattern, mesh_cache): (&'w World, &'w WgpuTileViewPattern, &'w GlobeTileMeshCache),
    pass: &mut TrackedRenderPass<'w>,
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
    ctx: (&'w World, &'w WgpuTileViewPattern, &'w GlobeTileMeshCache),
    pass: &mut TrackedRenderPass<'w>,
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
    pass: &mut TrackedRenderPass<'w>,
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
    let Some((Initialized(pattern), mesh_cache)) = world
        .resources
        .query::<(&Eventually<WgpuTileViewPattern>, &GlobeTileMeshCache)>()
    else {
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
