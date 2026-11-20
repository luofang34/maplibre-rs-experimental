use crate::{
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        render_phase::{
            PhaseItem, ProjectionBinding, RenderCommand, RenderCommandResult, TranslucentItem,
        },
    },
    sdf::{textures::SymbolTextures, SymbolBufferPool, SymbolModePipelines, SymbolPipeline},
    tcs::world::World,
};

pub struct SetSymbolPipeline;
impl<P: PhaseItem> RenderCommand<P> for SetSymbolPipeline {
    fn render<'w>(
        world: &'w World,
        _item: &P,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some((Initialized(symbol_pipeline), Initialized(projection_resources))) =
            world.resources.query::<(
                &Eventually<SymbolPipeline>,
                &Eventually<ProjectionGpuResources>,
            )>()
        else {
            return RenderCommandResult::Failure;
        };
        let Some(_) = symbol_pipeline
            .for_globe(projection_resources.is_globe_active(ProjectionBinding::View))
        else {
            return RenderCommandResult::Failure;
        };

        pass.set_bind_group(0, projection_resources.bind_group(), &[]);
        let Some(Initialized(depth)) = world
            .resources
            .get::<Eventually<super::depth::SymbolDepth>>()
        else {
            return RenderCommandResult::Failure;
        };
        pass.set_bind_group(2, &depth.binding, &[]);
        RenderCommandResult::Success
    }
}

pub struct DrawSymbol;
impl RenderCommand<TranslucentItem> for DrawSymbol {
    fn render<'w>(
        world: &'w World,
        item: &TranslucentItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some((
            Initialized(symbol_buffer_pool),
            covering,
            Initialized(pipeline),
            Initialized(projection_resources),
        )) = world.resources.query::<(
            &Eventually<SymbolBufferPool>,
            &super::covering::SymbolCovering,
            &Eventually<SymbolPipeline>,
            &Eventually<ProjectionGpuResources>,
        )>()
        else {
            return RenderCommandResult::Failure;
        };
        let Some(mode) =
            pipeline.for_globe(projection_resources.is_globe_active(ProjectionBinding::View))
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

        let Some(binding) = world
            .resources
            .get::<SymbolTextures>()
            .and_then(|textures| textures.binding_by_id(item.tile.coords, item.style_layer))
        else {
            return RenderCommandResult::Failure;
        };
        pass.set_bind_group(1, &binding.group, &[]);

        let index_range = entry.indices_buffer_range();

        if index_range.is_empty() {
            tracing::error!("Tried to draw a vector tile without any vertices");
            return RenderCommandResult::Failure;
        }

        let whole = world
            .resources
            .get::<crate::render::tracked_pass::DrawCapabilities>()
            .is_some_and(|capabilities| capabilities.base_vertex);
        let (first, base_vertex) = symbol_buffer_pool.bind_layer(pass, entry, (2, whole));
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

pub type DrawSymbols = (SetSymbolPipeline, DrawSymbol);
