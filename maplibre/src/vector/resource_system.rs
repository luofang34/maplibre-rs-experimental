//! Prepares GPU-owned resources by initializing them if they are uninitialized or out-of-date.
use crate::{
    context::MapContext,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        resource::{RenderPipeline, TilePipeline},
        shaders,
        shaders::Shader,
        RenderResources, Renderer,
    },
    tcs::system::{SystemError, SystemResult},
    vector::{
        resource::BufferPool, CirclePipeline, LinePipeline, VectorBufferPool, VectorPipeline,
    },
};

pub fn resource_system(
    MapContext {
        world,
        style,
        view_state,
        renderer:
            Renderer {
                device,
                queue,
                resources: RenderResources { surface, .. },
                settings,
                ..
            },
        ..
    }: &mut MapContext,
) -> SystemResult {
    if world
        .resources
        .get::<super::line_dash::LineDashResources>()
        .is_none()
    {
        world
            .resources
            .insert(super::line_dash::LineDashResources::new(device, queue));
    }
    if let Some(dashes) = world
        .resources
        .get_mut::<super::line_dash::LineDashResources>()
    {
        dashes.update(device, queue, style, view_state.style_zoom().value());
    }
    let Some((
        buffer_pool,
        vector_pipeline,
        line_pipeline,
        circle_pipeline,
        Initialized(projection_resources),
        dashes,
    )) = world.resources.query_mut::<(
        &mut Eventually<VectorBufferPool>,
        &mut Eventually<VectorPipeline>,
        &mut Eventually<LinePipeline>,
        &mut Eventually<CirclePipeline>,
        &mut Eventually<ProjectionGpuResources>,
        &super::line_dash::LineDashResources,
    )>()
    else {
        return Err(SystemError::Dependencies);
    };

    buffer_pool.initialize(|| BufferPool::from_device_with_sizes(device, settings.buffer_pools));

    let setup = PipelineSetup {
        device,
        settings: *settings,
        format: surface.surface_format(),
        multisampling: surface.is_multisampling_supported(settings.msaa),
        projection: projection_resources.bind_group_layout(),
    };
    setup.initialize(
        vector_pipeline,
        line_pipeline,
        circle_pipeline,
        &dashes.layout,
    );
    Ok(())
}

struct PipelineSetup<'a> {
    device: &'a wgpu::Device,
    settings: crate::render::settings::RendererSettings,
    format: wgpu::TextureFormat,
    multisampling: bool,
    projection: &'a wgpu::BindGroupLayout,
}

impl PipelineSetup<'_> {
    fn create(
        &self,
        name: &'static str,
        shader: &impl Shader,
        layouts: &[&wgpu::BindGroupLayout],
        depth: bool,
    ) -> wgpu::RenderPipeline {
        let pipeline = TilePipeline::new(
            name.into(),
            self.settings,
            shader.describe_vertex(),
            shader.describe_fragment(),
            true,
            false,
            false,
            false,
            self.multisampling,
            false,
            false,
        );
        let pipeline = if depth {
            pipeline.with_depth_write()
        } else {
            pipeline
        };
        pipeline
            .describe_render_pipeline()
            .initialize_with_prefix_layouts(self.device, layouts)
    }

    fn initialize(
        &self,
        vector: &mut Eventually<VectorPipeline>,
        line: &mut Eventually<LinePipeline>,
        circle: &mut Eventually<CirclePipeline>,
        dashes: &wgpu::BindGroupLayout,
    ) {
        vector.initialize(|| {
            VectorPipeline(self.create(
                "vector_pipeline",
                &shaders::FillShader {
                    format: self.format,
                },
                &[self.projection],
                false,
            ))
        });
        line.initialize(|| {
            let shader = shaders::LineShader {
                format: self.format,
            };
            LinePipeline(
                self.create("line_pipeline", &shader, &[self.projection, dashes], false),
                self.create(
                    "spatial_line_pipeline",
                    &shader,
                    &[self.projection, dashes],
                    true,
                ),
            )
        });
        circle.initialize(|| {
            CirclePipeline(self.create(
                "circle_pipeline",
                &shaders::CircleShader {
                    format: self.format,
                },
                &[self.projection],
                false,
            ))
        });
    }
}
