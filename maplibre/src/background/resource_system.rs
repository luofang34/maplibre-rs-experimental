//! GPU pipeline initialization for background paint, sky and atmosphere.

use crate::{
    context::MapContext,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        resource::{RenderPipeline, TilePipeline},
        shaders::{
            AtmosphereShader, BackgroundPatternShader, BackgroundShader,
            GlobeBackgroundPatternShader, GlobeBackgroundShader, Shader, SkyShader,
        },
    },
};

/// Initializes missing pipelines using the surface format and supported multisampling.
/// Returns `Dependencies` unless all pipeline slots and initialized projection resources
/// are present. Already initialized pipelines are retained.
pub fn resource_system(
    MapContext {
        world,
        style,
        view_state,
        renderer:
            crate::render::Renderer {
                device,
                queue,
                resources: crate::render::RenderResources { surface, .. },
                settings,
                ..
            },
        ..
    }: &mut MapContext,
) -> crate::tcs::system::SystemResult {
    if world
        .resources
        .get::<super::pattern::BackgroundPatternGpu>()
        .is_none()
    {
        let shader = BackgroundPatternShader {
            format: surface.surface_format(),
        };
        let layouts = super::pattern::layouts(device);
        let pipeline = TilePipeline::new(
            "background_pattern_pipeline".into(),
            *settings,
            shader.describe_vertex(),
            shader.describe_fragment(),
            crate::render::resource::TilePipelineOptions {
                depth_stencil_enabled: true,
                update_stencil: false,
                debug_stencil: true,
                wireframe: false,
                multisampling: surface.is_multisampling_supported(settings.msaa),
                textured: false,
            },
        )
        .describe_render_pipeline()
        .initialize_with_prefix_layouts(device, &[&layouts[0], &layouts[1]]);
        if world
            .resources
            .get::<crate::vector::pattern::PatternResources>()
            .is_none()
        {
            world
                .resources
                .insert(crate::vector::pattern::PatternResources::new(
                    device,
                    layouts[0].clone(),
                ));
        }
        world
            .resources
            .insert(super::pattern::BackgroundPatternGpu::new(
                device,
                queue,
                pipeline,
                &layouts[1],
            ));
    }
    if let Some(patterns) = world
        .resources
        .get_mut::<crate::vector::pattern::PatternResources>()
    {
        patterns.update(device, queue, style, view_state.style_zoom().value());
    }
    if world
        .resources
        .get::<super::pattern::BackgroundPatternGpu>()
        .is_some_and(|gpu| !gpu.has_globe())
    {
        let projection_layout = match world.resources.get::<Eventually<ProjectionGpuResources>>() {
            Some(Initialized(resources)) => Some(resources.bind_group_layout().clone()),
            _ => None,
        };
        if let Some(projection_layout) = projection_layout {
            let shader = GlobeBackgroundPatternShader {
                format: surface.surface_format(),
            };
            let image_layouts = super::pattern::layouts(device);
            let world_layout = super::pattern::world_layout(device);
            let pipeline = TilePipeline::new(
                "globe_background_pattern_pipeline".into(),
                *settings,
                shader.describe_vertex(),
                shader.describe_fragment(),
                crate::render::resource::TilePipelineOptions {
                    depth_stencil_enabled: true,
                    update_stencil: false,
                    debug_stencil: true,
                    wireframe: false,
                    multisampling: surface.is_multisampling_supported(settings.msaa),
                    textured: false,
                },
            )
            .with_depth_write()
            .describe_render_pipeline()
            .initialize_with_prefix_layouts(
                device,
                &[&projection_layout, &image_layouts[0], &world_layout],
            );
            if let Some(gpu) = world
                .resources
                .get_mut::<super::pattern::BackgroundPatternGpu>()
            {
                gpu.with_globe(device, queue, pipeline, &world_layout);
            }
        }
    }
    let Some((
        background_pipeline,
        globe_background_pipeline,
        atmosphere_pipeline,
        sky_pipeline,
        Initialized(projection_resources),
    )) = world.resources.query_mut::<(
        &mut Eventually<BackgroundRenderPipeline>,
        &mut Eventually<GlobeBackgroundRenderPipeline>,
        &mut Eventually<AtmosphereRenderPipeline>,
        &mut Eventually<SkyRenderPipeline>,
        &mut Eventually<ProjectionGpuResources>,
    )>()
    else {
        return Err(crate::tcs::system::SystemError::Dependencies);
    };

    background_pipeline.initialize(|| {
        let shader = BackgroundShader {
            format: surface.surface_format(),
        };

        let pipeline = TilePipeline::new(
            "background_pipeline".into(),
            *settings,
            shader.describe_vertex(),
            shader.describe_fragment(),
            crate::render::resource::TilePipelineOptions {
                depth_stencil_enabled: true,
                update_stencil: false,
                debug_stencil: true,
                wireframe: false,
                multisampling: surface.is_multisampling_supported(settings.msaa),
                textured: false,
            },
        )
        .describe_render_pipeline()
        .initialize(device);

        BackgroundRenderPipeline(pipeline)
    });

    globe_background_pipeline.initialize(|| {
        let shader = GlobeBackgroundShader {
            format: surface.surface_format(),
        };
        let pipeline = TilePipeline::new(
            "globe_background_pipeline".into(),
            *settings,
            shader.describe_vertex(),
            shader.describe_fragment(),
            crate::render::resource::TilePipelineOptions {
                depth_stencil_enabled: true,
                update_stencil: false,
                debug_stencil: true,
                wireframe: false,
                multisampling: surface.is_multisampling_supported(settings.msaa),
                textured: false,
            },
        )
        .with_depth_write()
        .describe_render_pipeline()
        .initialize_with_prefix_layouts(device, &[projection_resources.bind_group_layout()]);
        GlobeBackgroundRenderPipeline(pipeline)
    });

    atmosphere_pipeline.initialize(|| {
        let shader = AtmosphereShader {
            format: surface.surface_format(),
        };
        let pipeline = TilePipeline::new(
            "atmosphere_pipeline".into(),
            *settings,
            shader.describe_vertex(),
            shader.describe_fragment(),
            crate::render::resource::TilePipelineOptions {
                depth_stencil_enabled: true,
                update_stencil: false,
                debug_stencil: true,
                wireframe: false,
                multisampling: surface.is_multisampling_supported(settings.msaa),
                textured: false,
            },
        )
        .describe_render_pipeline()
        .initialize_with_prefix_layouts(device, &[projection_resources.bind_group_layout()]);
        AtmosphereRenderPipeline(pipeline)
    });

    sky_pipeline.initialize(|| {
        let shader = SkyShader {
            format: surface.surface_format(),
        };
        // The host compositor needs depth for the opaque sky as well as terrain.
        let pipeline = TilePipeline::new(
            "sky_pipeline".into(),
            *settings,
            shader.describe_vertex(),
            shader.describe_fragment(),
            crate::render::resource::TilePipelineOptions {
                depth_stencil_enabled: true,
                update_stencil: false,
                debug_stencil: true,
                wireframe: false,
                multisampling: surface.is_multisampling_supported(settings.msaa),
                textured: false,
            },
        )
        .with_depth_write()
        .describe_render_pipeline()
        .initialize_with_prefix_layouts(device, &[projection_resources.bind_group_layout()]);
        SkyRenderPipeline(pipeline)
    });

    Ok(())
}

/// Pipeline drawing a flat fullscreen background.
pub struct BackgroundRenderPipeline(pub wgpu::RenderPipeline);

/// Pipeline drawing background paint on a globe mesh.
pub struct GlobeBackgroundRenderPipeline(pub wgpu::RenderPipeline);

/// Pipeline drawing optional atmospheric scattering.
pub struct AtmosphereRenderPipeline(pub wgpu::RenderPipeline);

/// Pipeline filling the screen above the horizon with the sky.
pub struct SkyRenderPipeline(pub wgpu::RenderPipeline);
