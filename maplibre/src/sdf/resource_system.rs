//! Creates the symbol pipeline and geometry pools.
use crate::{
    context::MapContext,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        resource::{RenderPipeline, TilePipeline},
        shaders::{Shader, SymbolShader},
    },
    sdf::{SymbolBufferPool, SymbolPipeline},
    tcs::system::{SystemError, SystemResult},
    vector::resource::BufferPool,
};

pub fn resource_system(
    MapContext {
        world, renderer, ..
    }: &mut MapContext,
) -> SystemResult {
    let Some((pool, pipeline, Initialized(projection))) = world.resources.query_mut::<(
        &mut Eventually<SymbolBufferPool>,
        &mut Eventually<SymbolPipeline>,
        &mut Eventually<ProjectionGpuResources>,
    )>() else {
        return Err(SystemError::Dependencies);
    };
    pool.initialize(|| {
        BufferPool::from_device_with_sizes(&renderer.device, renderer.settings.symbol_pools)
    });
    pipeline.initialize(|| {
        let surface = &renderer.resources.surface;
        let shader = SymbolShader {
            format: surface.surface_format(),
        };
        let mut descriptor = TilePipeline::new(
            "symbol_pipeline".into(),
            renderer.settings,
            shader.describe_vertex(),
            shader.describe_fragment(),
            true,
            false,
            true,
            false,
            surface.is_multisampling_supported(renderer.settings.msaa),
            false,
            true,
        )
        .with_depth_write()
        .describe_render_pipeline();
        if let Some(layout) = descriptor
            .layout
            .as_mut()
            .and_then(|groups| groups.first_mut())
        {
            layout.push(wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            });
        }
        descriptor
            .layout
            .get_or_insert_with(Vec::new)
            .push(super::depth::SymbolDepth::layout());
        if let Some(depth) = descriptor.depth_stencil.as_mut() {
            depth.depth_compare = wgpu::CompareFunction::Always;
        }
        SymbolPipeline(
            descriptor.initialize_with_prefix_layouts(
                &renderer.device,
                &[projection.bind_group_layout()],
            ),
        )
    });
    let Some((depth, Initialized(pipeline))) = world.resources.query_mut::<(
        &mut Eventually<super::depth::SymbolDepth>,
        &Eventually<SymbolPipeline>,
    )>() else {
        return Err(SystemError::Dependencies);
    };
    let Initialized(source) = &renderer.resources.depth_texture else {
        return Err(SystemError::Dependencies);
    };
    let size = source.texture.size();
    let samples = source.texture.sample_count();
    depth.reinitialize(
        || super::depth::SymbolDepth::new(&renderer.device, size, samples, pipeline),
        &(size.width, size.height, samples),
    );
    Ok(())
}
