use std::ops::Range;

use super::BinnedPhaseItem;
use crate::{
    render::{
        render_phase::{
            draw::{PhaseItemExtraIndex, RenderCommandResult},
            indirect_buffers::IndirectParametersBuffers,
        },
        tracked_pass::{DrawCapabilities, TrackedRenderPass},
    },
    tcs::world::World,
};

/// Geometry range supplied to [`draw_binned_mesh`] when recording a terminal draw command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DirectDrawGeometry {
    /// Indexed geometry (`indices` range with `base_vertex` offset).
    Indexed {
        /// Contiguous index range in the bound index buffer.
        indices: Range<u32>,
        /// Signed vertex offset added to each index.
        base_vertex: i32,
    },
    /// Non-indexed geometry (`vertices` range).
    NonIndexed {
        /// Contiguous vertex range in the bound vertex buffer(s).
        vertices: Range<u32>,
    },
}

/// Dispatches the terminal GPU draw call for `item`, automatically routing between direct
/// instancing, single indirect draw, multi-draw indirect, and GPU-counted multi-draw indirect.
pub fn draw_binned_mesh<'w, BPI: BinnedPhaseItem>(
    pass: &mut TrackedRenderPass<'w>,
    world: &'w World,
    item: &BPI,
    geometry: DirectDrawGeometry,
) -> RenderCommandResult {
    match item.extra_index() {
        PhaseItemExtraIndex::None | PhaseItemExtraIndex::DynamicOffset(_) => {
            draw_direct_instances(pass, item.batch_range().clone(), geometry, true)
        }
        PhaseItemExtraIndex::VertexInstance => {
            let range = item.batch_range();
            let count = range.end.saturating_sub(range.start);
            let has_base_vertex = world
                .resources
                .get::<DrawCapabilities>()
                .is_none_or(|caps| caps.base_vertex);
            draw_direct_instances(pass, 0..count, geometry, has_base_vertex)
        }
        PhaseItemExtraIndex::IndirectParametersIndex {
            range,
            batch_set_index,
        } => draw_indirect_mesh(pass, world, range, batch_set_index, geometry),
    }
}

fn draw_direct_instances<'w>(
    pass: &mut TrackedRenderPass<'w>,
    instances: Range<u32>,
    geometry: DirectDrawGeometry,
    has_base_vertex: bool,
) -> RenderCommandResult {
    if instances.is_empty() {
        return RenderCommandResult::Success;
    }
    match geometry {
        DirectDrawGeometry::Indexed {
            indices,
            base_vertex,
        } => {
            let effective_base_vertex = if has_base_vertex { base_vertex } else { 0 };
            pass.draw_indexed(indices, effective_base_vertex, instances);
        }
        DirectDrawGeometry::NonIndexed { vertices } => pass.draw(vertices, instances),
    }
    RenderCommandResult::Success
}

fn draw_indirect_mesh<'w>(
    pass: &mut TrackedRenderPass<'w>,
    world: &'w World,
    range: Range<u32>,
    batch_set_index: Option<std::num::NonZeroU32>,
    geometry: DirectDrawGeometry,
) -> RenderCommandResult {
    if range.is_empty() {
        return RenderCommandResult::Success;
    }
    let Some(indirect_buffers) = world.resources.get::<IndirectParametersBuffers>() else {
        return RenderCommandResult::Failure;
    };
    let count = range.end.saturating_sub(range.start);
    let use_gpu_count = batch_set_index.is_some()
        && indirect_buffers.gpu_count_enabled()
        && world
            .resources
            .get::<DrawCapabilities>()
            .is_some_and(|caps| caps.multi_draw_indirect_count);
    let indexed = matches!(geometry, DirectDrawGeometry::Indexed { .. });
    let (Some(buffer), offset) = (if indexed {
        (
            indirect_buffers.indexed_buffer(),
            IndirectParametersBuffers::indexed_byte_offset(range.start),
        )
    } else {
        (
            indirect_buffers.non_indexed_buffer(),
            IndirectParametersBuffers::non_indexed_byte_offset(range.start),
        )
    }) else {
        return RenderCommandResult::Failure;
    };
    if let (true, Some(bs_idx), Some(count_buf)) = (
        use_gpu_count,
        batch_set_index,
        indirect_buffers.batch_sets_buffer(),
    ) {
        let count_offset = IndirectParametersBuffers::batch_set_count_byte_offset(bs_idx);
        if indexed {
            pass.multi_draw_indexed_indirect_count(buffer, offset, count_buf, count_offset, count);
        } else {
            pass.multi_draw_indirect_count(buffer, offset, count_buf, count_offset, count);
        }
    } else if count > 1 {
        if indexed {
            pass.multi_draw_indexed_indirect(buffer, offset, count);
        } else {
            pass.multi_draw_indirect(buffer, offset, count);
        }
    } else if indexed {
        pass.draw_indexed_indirect(buffer, offset);
    } else {
        pass.draw_indirect(buffer, offset);
    }
    RenderCommandResult::Success
}

/// Convenience wrapper over [`draw_binned_mesh`] for indexed meshes.
#[inline]
pub fn draw_binned_indexed<'w, BPI: BinnedPhaseItem>(
    pass: &mut TrackedRenderPass<'w>,
    world: &'w World,
    item: &BPI,
    indices: Range<u32>,
    base_vertex: i32,
) -> RenderCommandResult {
    draw_binned_mesh(
        pass,
        world,
        item,
        DirectDrawGeometry::Indexed {
            indices,
            base_vertex,
        },
    )
}

/// Convenience wrapper over [`draw_binned_mesh`] for non-indexed meshes.
#[inline]
pub fn draw_binned_non_indexed<'w, BPI: BinnedPhaseItem>(
    pass: &mut TrackedRenderPass<'w>,
    world: &'w World,
    item: &BPI,
    vertices: Range<u32>,
) -> RenderCommandResult {
    draw_binned_mesh(
        pass,
        world,
        item,
        DirectDrawGeometry::NonIndexed { vertices },
    )
}
