use crate::{
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        render_phase::{PhaseItem, RenderCommand, RenderCommandResult, TranslucentItem},
    },
    sdf::{textures::SymbolTextures, SymbolBufferPool, SymbolPipeline},
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

        pass.set_pipeline(&symbol_pipeline.combined);
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
        let Some((Initialized(symbol_buffer_pool), covering, Initialized(pipeline))) =
            world.resources.query::<(
                &Eventually<SymbolBufferPool>,
                &super::covering::SymbolCovering,
                &Eventually<SymbolPipeline>,
            )>()
        else {
            return RenderCommandResult::Failure;
        };

        let Some(vector_layers) = symbol_buffer_pool.index().get_layers(item.tile.coords) else {
            return RenderCommandResult::Failure;
        };

        let Some(entry) = vector_layers
            .iter()
            .find(|entry| entry.style_layer.id == item.style_layer)
        else {
            return RenderCommandResult::Failure;
        };

        let Some(binding) = world
            .resources
            .get::<SymbolTextures>()
            .and_then(|textures| textures.binding(item.tile.coords, &item.style_layer))
        else {
            return RenderCommandResult::Failure;
        };
        pass.set_bind_group(1, &binding.group, &[]);

        let Some(tile_view_pattern_buffer) = item.source_shape.buffer_range() else {
            return RenderCommandResult::Failure;
        };

        let reference = item.source_shape.coords().stencil_reference_value_3d() as u32;

        let index_range = entry.indices_buffer_range();

        if index_range.is_empty() {
            tracing::error!("Tried to draw a vector tile without any vertices");
            return RenderCommandResult::Failure;
        }

        pass.set_stencil_reference(reference);

        let whole = world
            .resources
            .get::<crate::render::tracked_pass::DrawCapabilities>()
            .is_some_and(|capabilities| capabilities.base_vertex);
        let (first, base_vertex) = symbol_buffer_pool.bind_layer(pass, entry, (3, whole));
        pass.set_vertex_buffer(1, covering.buffer.slice(tile_view_pattern_buffer));
        pass.set_vertex_buffer(
            2,
            symbol_buffer_pool
                .metadata()
                .slice(entry.layer_metadata_buffer_range()),
        );
        draw_indices(
            pass,
            pipeline,
            (
                first..first + entry.indices_range().len() as u32,
                base_vertex,
            ),
            binding.separate_halo,
        );
        RenderCommandResult::Success
    }
}

fn draw_indices<'w>(
    pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    pipeline: &'w SymbolPipeline,
    (indices, base_vertex): (std::ops::Range<u32>, i32),
    separate_halo: bool,
) {
    if separate_halo {
        // All halos must precede fills so an adjacent glyph cannot cover a finished stroke.
        pass.set_pipeline(&pipeline.halo);
        pass.draw_indexed(indices.clone(), base_vertex, 0..1);
        pass.set_pipeline(&pipeline.fill);
    }
    pass.draw_indexed(indices, base_vertex, 0..1);
}

pub type DrawSymbols = (SetSymbolPipeline, DrawSymbol);
