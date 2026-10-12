//! Creates the terrain pipeline once a style declares terrain.

use crate::{
    context::MapContext,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        resource::{RenderPipeline, TilePipeline},
        settings::Msaa,
        shaders::{Shader, TerrainShader},
        RenderResources, Renderer,
    },
    tcs::system::{SystemError, SystemResult},
    terrain::resources::TerrainResources,
};

pub fn resource_system(
    MapContext {
        style,
        world,
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
    if style.terrain.is_none() {
        return Ok(());
    }
    let Some((terrain_resources, Initialized(projection_resources))) =
        world.resources.query_mut::<(
            &mut Eventually<TerrainResources>,
            &mut Eventually<ProjectionGpuResources>,
        )>()
    else {
        return Err(SystemError::Dependencies);
    };

    terrain_resources.initialize(|| {
        let msaa = if surface.is_multisampling_supported(settings.msaa) {
            settings.msaa
        } else {
            Msaa { samples: 1 }
        };
        let shader = TerrainShader {
            format: surface.surface_format(),
        };
        let mut descriptor = TilePipeline::new(
            "terrain_pipeline".into(),
            *settings,
            shader.describe_vertex(),
            shader.describe_fragment(),
            crate::render::resource::TilePipelineOptions {
                depth_stencil_enabled: true,
                update_stencil: false,
                debug_stencil: true,
                wireframe: false,
                multisampling: msaa.is_multisampling(),
                textured: false,
            },
        )
        .with_depth_write()
        .describe_render_pipeline();
        descriptor.primitive.cull_mode = Some(wgpu::Face::Back);
        descriptor.layout = Some(vec![TerrainResources::bind_group_layout_entries()]);
        let pipeline = descriptor
            .initialize_with_prefix_layouts(device, &[projection_resources.bind_group_layout()]);
        TerrainResources::new(
            device,
            queue,
            pipeline,
            surface.surface_format(),
            settings.depth_texture_format,
            msaa,
            settings.terrain_drape_size,
        )
    });
    Ok(())
}
