//! Builds the density and composite pipelines once the renderer is up.

use crate::{
    context::MapContext,
    heatmap::resources::HeatmapResources,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        resource::{RenderPipeline, TilePipeline, TilePipelineOptions},
        shaders::{HeatmapCompositeShader, HeatmapDensityShader, Shader},
        RenderResources, Renderer,
    },
    tcs::system::{SystemError, SystemResult},
};

/// Bind group of the composite's ramp texture and opacity, after the density texture group.
fn ramp_layout() -> Vec<wgpu::BindGroupLayoutEntry> {
    vec![
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                multisampled: false,
                view_dimension: wgpu::TextureViewDimension::D2,
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
    ]
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
    let Some((heatmap_resources, Initialized(projection_resources))) =
        world.resources.query_mut::<(
            &mut Eventually<HeatmapResources>,
            &mut Eventually<ProjectionGpuResources>,
        )>()
    else {
        return Err(SystemError::Dependencies);
    };

    heatmap_resources.initialize(|| {
        let shader = HeatmapDensityShader;
        // The density target has no depth or stencil, one sample and no tile clipping: every
        // point is drawn once by the tile that owns it.
        let density = TilePipeline::new(
            "heatmap_density_pipeline".into(),
            *settings,
            shader.describe_vertex(),
            shader.describe_fragment(),
            TilePipelineOptions::default(),
        )
        .describe_render_pipeline()
        .initialize_with_prefix_layouts(device, &[projection_resources.bind_group_layout()]);

        let shader = HeatmapCompositeShader {
            format: surface.surface_format(),
        };
        let mut composite = TilePipeline::new(
            "heatmap_composite_pipeline".into(),
            *settings,
            shader.describe_vertex(),
            shader.describe_fragment(),
            TilePipelineOptions {
                depth_stencil_enabled: true,
                // Always pass: the composite covers the viewport, not a tile.
                debug_stencil: true,
                multisampling: surface.is_multisampling_supported(settings.msaa),
                textured: true,
                ..Default::default()
            },
        )
        .describe_render_pipeline();
        composite
            .layout
            .get_or_insert_with(Vec::new)
            .push(ramp_layout());
        let composite = composite.initialize_with_prefix_layouts(device, &[]);
        HeatmapResources::new(device, density, composite)
    });
    Ok(())
}
