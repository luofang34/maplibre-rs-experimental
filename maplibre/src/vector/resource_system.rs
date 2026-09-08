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
        dashes.update(device, queue, style, view_state.zoom().value());
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

    vector_pipeline.initialize(|| {
        let tile_shader = shaders::FillShader {
            format: surface.surface_format(),
        };

        let pipeline = TilePipeline::new(
            "vector_pipeline".into(),
            *settings,
            tile_shader.describe_vertex(),
            tile_shader.describe_fragment(),
            true,
            false,
            false,
            false,
            surface.is_multisampling_supported(settings.msaa),
            false,
            false,
        )
        .describe_render_pipeline()
        .initialize_with_prefix_layouts(device, &[projection_resources.bind_group_layout()]);

        VectorPipeline(pipeline)
    });

    line_pipeline.initialize(|| {
        let line_shader = shaders::LineShader {
            format: surface.surface_format(),
        };

        let pipeline = TilePipeline::new(
            "line_pipeline".into(),
            *settings,
            line_shader.describe_vertex(),
            line_shader.describe_fragment(),
            true,
            false,
            false,
            false,
            surface.is_multisampling_supported(settings.msaa),
            false,
            false,
        )
        .describe_render_pipeline()
        .initialize_with_prefix_layouts(
            device,
            &[projection_resources.bind_group_layout(), &dashes.layout],
        );

        LinePipeline(pipeline)
    });

    circle_pipeline.initialize(|| {
        let circle_shader = shaders::CircleShader {
            format: surface.surface_format(),
        };

        let pipeline = TilePipeline::new(
            "circle_pipeline".into(),
            *settings,
            circle_shader.describe_vertex(),
            circle_shader.describe_fragment(),
            true,
            false,
            false,
            false,
            surface.is_multisampling_supported(settings.msaa),
            false,
            false,
        )
        .describe_render_pipeline()
        .initialize_with_prefix_layouts(device, &[projection_resources.bind_group_layout()]);

        CirclePipeline(pipeline)
    });

    Ok(())
}
