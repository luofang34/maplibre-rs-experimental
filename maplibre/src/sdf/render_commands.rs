use crate::{
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        render_phase::{ProjectionBinding, RenderCommand, RenderCommandResult, TranslucentItem},
    },
    sdf::{textures::SymbolTextures, SymbolBufferPool, SymbolModePipelines, SymbolPipeline},
    tcs::world::World,
};

fn draw_indices<'w>(
    pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    pipeline: &'w SymbolModePipelines,
    (indices, base_vertex, instances): (std::ops::Range<u32>, i32, std::ops::Range<u32>),
    separate_halo: bool,
) -> RenderCommandResult {
    if separate_halo {
        let (Some(halo), Some(fill)) = (&pipeline.halo, &pipeline.fill) else {
            return RenderCommandResult::Failure;
        };
        // All halos must precede fills so an adjacent glyph cannot cover a finished stroke.
        pass.set_pipeline(halo);
        pass.draw_indexed(indices.clone(), base_vertex, instances.clone());
        pass.set_pipeline(fill);
        pass.draw_indexed(indices, base_vertex, instances);
    } else {
        let Some(combined) = &pipeline.combined else {
            return RenderCommandResult::Failure;
        };
        pass.set_pipeline(combined);
        pass.draw_indexed(indices, base_vertex, instances);
    }
    RenderCommandResult::Success
}

/// Fused symbol render command binding projection, depth, atlas texture, and drawing indices in
/// one resource query.
pub struct DrawSymbols;

impl RenderCommand<TranslucentItem> for DrawSymbols {
    fn render<'w>(
        world: &'w World,
        item: &TranslucentItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some((
            Initialized(symbol_pipeline),
            Initialized(projection_resources),
            Initialized(depth),
            Initialized(symbol_buffer_pool),
            covering,
            textures,
        )) = world.resources.query::<(
            &Eventually<SymbolPipeline>,
            &Eventually<ProjectionGpuResources>,
            &Eventually<super::depth::SymbolDepth>,
            &Eventually<SymbolBufferPool>,
            &super::covering::SymbolCovering,
            &SymbolTextures,
        )>()
        else {
            return RenderCommandResult::Failure;
        };
        let Some(mode) = symbol_pipeline
            .for_globe(projection_resources.is_globe_active(ProjectionBinding::View))
        else {
            return RenderCommandResult::Failure;
        };
        let Some(entry) = symbol_buffer_pool.index().resolve_entry(
            item.tile.coords,
            item.style_layer,
            item.entry_slot,
        ) else {
            return RenderCommandResult::Failure;
        };
        let Some(binding) = textures.binding_by_id(item.tile.coords, item.style_layer) else {
            return RenderCommandResult::Failure;
        };
        if entry.indices_buffer_range().is_empty() {
            tracing::error!("Tried to draw a vector tile without any vertices");
            return RenderCommandResult::Failure;
        }
        let whole = pass.base_vertex(world);
        let instances = if whole {
            let Some(tile_instance) = item.source_shape.instance_index() else {
                return RenderCommandResult::Failure;
            };
            pass.set_vertex_buffer(1, covering.buffer.slice(..));
            tile_instance..tile_instance + 1
        } else {
            let Some(tile_view_pattern_buffer) = item.source_shape.buffer_range() else {
                return RenderCommandResult::Failure;
            };
            pass.set_vertex_buffer(1, covering.buffer.slice(tile_view_pattern_buffer));
            0..1
        };
        pass.set_bind_group(0, projection_resources.bind_group(), &[]);
        pass.set_bind_group(1, &binding.group, &[]);
        pass.set_bind_group(2, &depth.binding, &[]);
        let (first, base_vertex) = symbol_buffer_pool.bind_layer(pass, entry, (2, whole));
        draw_indices(
            pass,
            mode,
            (
                first..first + entry.indices_range().len() as u32,
                base_vertex,
                instances,
            ),
            binding.separate_halo,
        )
    }
}
