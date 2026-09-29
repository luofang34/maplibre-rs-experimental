//! Prepares GPU-owned resources by initializing them if they are uninitialized or out-of-date.
use crate::{
    context::MapContext,
    raster::resource::RasterResources,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        resource::{RenderPipeline, TilePipeline},
        settings::Msaa,
        shaders,
        shaders::Shader,
        RenderResources, Renderer,
    },
    tcs::system::{SystemError, SystemResult},
};

pub fn resource_system(
    MapContext {
        world,
        style,
        renderer:
            Renderer {
                device,
                resources: RenderResources { surface, .. },
                settings,
                ..
            },
        ..
    }: &mut MapContext,
) -> SystemResult {
    let Some((raster_resources, Initialized(projection_resources))) = world.resources.query_mut::<(
        &mut Eventually<RasterResources>,
        &mut Eventually<ProjectionGpuResources>,
    )>() else {
        return Err(SystemError::Dependencies);
    };

    raster_resources.initialize(|| {
        let shader = shaders::RasterShader {
            format: surface.surface_format(),
        };

        let mut descriptor = TilePipeline::new(
            "raster_pipeline".into(),
            *settings,
            shader.describe_vertex(),
            shader.describe_fragment(),
            crate::render::resource::TilePipelineOptions {
                depth_stencil_enabled: true,
                update_stencil: false,
                debug_stencil: false,
                wireframe: false,
                multisampling: surface.is_multisampling_supported(settings.msaa),
                textured: true,
            },
        )
        .describe_render_pipeline();
        if let Some(state) = &mut descriptor.depth_stencil {
            // Tile references occupy the low seven bits, so even root reference zero is consumed.
            state.stencil.front.pass_op = wgpu::StencilOperation::Invert;
            state.stencil.back.pass_op = wgpu::StencilOperation::Invert;
        }
        let pipeline = descriptor
            .initialize_with_prefix_layouts(device, &[projection_resources.bind_group_layout()]);
        RasterResources::new(Msaa { samples: 1 }, device, pipeline)
    });
    if let Initialized(resources) = raster_resources {
        resources.update_layer_sources(style);
    }
    Ok(())
}
