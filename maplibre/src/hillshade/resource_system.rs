//! Builds the two DEM pipelines once the renderer is up.

use crate::{
    context::MapContext,
    hillshade::resources::HillshadeResources,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        resource::{RenderPipeline, TilePipeline},
        shaders::{DemShader, DemShading, Shader},
        RenderResources, Renderer,
    },
    tcs::system::{SystemError, SystemResult},
};

/// Bind group layout of the per-layer uniforms, shared by both pipelines.
fn uniform_layout() -> Vec<wgpu::BindGroupLayoutEntry> {
    vec![wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }]
}

pub fn resource_system(
    MapContext {
        world,
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
    let Some((hillshade_resources, Initialized(projection_resources))) =
        world.resources.query_mut::<(
            &mut Eventually<HillshadeResources>,
            &mut Eventually<ProjectionGpuResources>,
        )>()
    else {
        return Err(SystemError::Dependencies);
    };

    hillshade_resources.initialize(|| {
        let build = |name: &'static str, shading: DemShading| {
            let shader = DemShader {
                format: surface.surface_format(),
                shading,
            };
            let mut descriptor = TilePipeline::new(
                name.into(),
                *settings,
                shader.describe_vertex(),
                shader.describe_fragment(),
                true,
                false,
                true,
                false,
                surface.is_multisampling_supported(settings.msaa),
                true,
                false,
            )
            .describe_render_pipeline();
            // The raster flag gives group 1 the tile texture; the layer uniforms follow.
            descriptor
                .layout
                .get_or_insert_with(Vec::new)
                .push(uniform_layout());
            descriptor
                .initialize_with_prefix_layouts(device, &[projection_resources.bind_group_layout()])
        };
        HillshadeResources::new(
            build("hillshade_pipeline", DemShading::Hillshade),
            build("color_relief_pipeline", DemShading::ColorRelief),
        )
    });
    Ok(())
}
