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

#[derive(Default)]
struct BackgroundStyleCache {
    key: Option<(u64, u64, u64)>,
    needs_bg_pattern: bool,
    has_opaque_bg: bool,
    has_blended_bg: bool,
}

impl BackgroundStyleCache {
    fn evaluate(
        &mut self,
        style: &crate::style::Style,
        zoom: f64,
        style_zoom: f64,
    ) -> (bool, bool, bool) {
        let key = (style.revision(), zoom.to_bits(), style_zoom.to_bits());
        if self.key == Some(key) {
            return (
                self.needs_bg_pattern,
                self.has_opaque_bg,
                self.has_blended_bg,
            );
        }
        let mut needs_bg_pattern = false;
        let (mut has_opaque_bg, mut has_blended_bg) = (false, false);
        for layer in &style.layers {
            if layer.type_ != "background" {
                continue;
            }
            let patterned = layer
                .paint
                .as_ref()
                .is_some_and(|p| crate::vector::pattern::pattern_name(p, style_zoom).is_some());
            needs_bg_pattern |= patterned;
            if patterned || !layer.is_visible_at(zoom) {
                continue;
            }
            if super::buffers::background_color(layer, zoom)[3] >= 1.0 {
                has_opaque_bg = true;
            } else {
                has_blended_bg = true;
            }
        }
        self.key = Some(key);
        self.needs_bg_pattern = needs_bg_pattern;
        self.has_opaque_bg = has_opaque_bg;
        self.has_blended_bg = has_blended_bg;
        (needs_bg_pattern, has_opaque_bg, has_blended_bg)
    }
}

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
    let zoom = view_state.zoom().value();
    let style_zoom = view_state.style_zoom().value();
    let (needs_bg_pattern, has_opaque_bg, has_blended_bg) = {
        let cache = world.resources.get_or_init_mut::<BackgroundStyleCache>();
        cache.evaluate(style, zoom, style_zoom)
    };
    let needs_globe = style
        .projection
        .as_ref()
        .is_some_and(|spec| spec.projection_type.globe_transition(zoom) > 0.0);
    let needs_sky = style.sky.is_some();
    let pattern_gpu_ready = !needs_bg_pattern
        || world
            .resources
            .get::<super::pattern::BackgroundPatternGpu>()
            .is_some_and(|gpu| !needs_globe || gpu.has_globe());
    let bg_ready = if needs_globe {
        world
            .resources
            .get::<Eventually<GlobeBackgroundRenderPipeline>>()
            .is_some_and(|p| match p {
                Eventually::Initialized(gp) => {
                    (!has_opaque_bg || gp.opaque.is_some())
                        && (!has_blended_bg || gp.blended.is_some())
                }
                Eventually::Uninitialized => false,
            })
    } else {
        world
            .resources
            .get::<Eventually<BackgroundRenderPipeline>>()
            .is_some_and(|p| match p {
                Eventually::Initialized(bp) => {
                    (!has_opaque_bg || (bp.solid_opaque.is_some() && bp.clipped_opaque.is_some()))
                        && (!has_blended_bg || (bp.solid.is_some() && bp.clipped.is_some()))
                }
                Eventually::Uninitialized => !has_opaque_bg && !has_blended_bg,
            })
    };
    let sky_ready = !needs_sky
        || matches!(
            world.resources.get::<Eventually<SkyRenderPipeline>>(),
            Some(Initialized(_))
        );
    let atmo_ready = match world
        .resources
        .get::<Eventually<AtmosphereRenderPipeline>>()
    {
        Some(Initialized(_)) => true,
        Some(Eventually::Uninitialized) => {
            super::queue_system::atmosphere_blend(style, view_state, world) <= 0.0
        }
        None => false,
    };
    if pattern_gpu_ready && bg_ready && sky_ready && atmo_ready {
        if world
            .resources
            .get::<Eventually<ProjectionGpuResources>>()
            .is_some_and(|p| matches!(p, Initialized(_)))
            && world
                .resources
                .get::<Eventually<BackgroundRenderPipeline>>()
                .is_some()
        {
            if let Some(patterns) = world
                .resources
                .get_mut::<crate::vector::pattern::PatternResources>()
            {
                patterns.update(device, queue, style, style_zoom);
            }
            return Ok(());
        }
    }
    if needs_bg_pattern
        && world
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
    if needs_globe
        && world
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
            let mut descriptor = TilePipeline::new(
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
            .describe_render_pipeline();
            descriptor.primitive.cull_mode = Some(wgpu::Face::Back);
            let pipeline = descriptor.initialize_with_prefix_layouts(
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
    let needs_atmosphere = world
        .resources
        .get::<Eventually<AtmosphereRenderPipeline>>()
        .is_none_or(|p| matches!(p, Eventually::Uninitialized))
        && super::queue_system::atmosphere_blend(style, view_state, world) > 0.0;
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

    let bg_pipelines = background_pipeline.initialize(BackgroundRenderPipeline::default);
    if !needs_globe {
        let size = surface.size();
        let covers = super::queue_system::flat_viewport_covered(
            view_state,
            style.terrain.is_none(),
            [size.width() as f32, size.height() as f32],
        );
        let shader = BackgroundShader {
            format: surface.surface_format(),
        };
        let options = crate::render::resource::TilePipelineOptions {
            depth_stencil_enabled: true,
            update_stencil: false,
            debug_stencil: true,
            wireframe: false,
            multisampling: surface.is_multisampling_supported(settings.msaa),
            textured: false,
        };
        if covers {
            if has_opaque_bg {
                bg_pipelines.solid_opaque.get_or_insert_with(|| {
                    TilePipeline::new(
                        "background_solid_opaque_pipeline".into(),
                        *settings,
                        shader.describe_vertex(),
                        shader.describe_solid_opaque_fragment(),
                        options,
                    )
                    .describe_render_pipeline()
                    .initialize(device)
                });
            }
            if has_blended_bg {
                bg_pipelines.solid.get_or_insert_with(|| {
                    TilePipeline::new(
                        "background_solid_pipeline".into(),
                        *settings,
                        shader.describe_vertex(),
                        shader.describe_solid_fragment(),
                        options,
                    )
                    .describe_render_pipeline()
                    .initialize(device)
                });
            }
        } else {
            if has_opaque_bg {
                bg_pipelines.clipped_opaque.get_or_insert_with(|| {
                    TilePipeline::new(
                        "background_clipped_opaque_pipeline".into(),
                        *settings,
                        shader.describe_vertex(),
                        shader.describe_clipped_opaque_fragment(),
                        options,
                    )
                    .describe_render_pipeline()
                    .initialize(device)
                });
            }
            if has_blended_bg {
                bg_pipelines.clipped.get_or_insert_with(|| {
                    TilePipeline::new(
                        "background_pipeline".into(),
                        *settings,
                        shader.describe_vertex(),
                        shader.describe_fragment(),
                        options,
                    )
                    .describe_render_pipeline()
                    .initialize(device)
                });
            }
        }
    }

    if needs_globe {
        let globe_bg = globe_background_pipeline.initialize(GlobeBackgroundRenderPipeline::default);
        let shader = GlobeBackgroundShader {
            format: surface.surface_format(),
        };
        let options = crate::render::resource::TilePipelineOptions {
            depth_stencil_enabled: true,
            update_stencil: false,
            debug_stencil: true,
            wireframe: false,
            multisampling: surface.is_multisampling_supported(settings.msaa),
            textured: false,
        };
        if has_opaque_bg {
            globe_bg.opaque.get_or_insert_with(|| {
                let mut descriptor = TilePipeline::new(
                    "globe_background_opaque_pipeline".into(),
                    *settings,
                    shader.describe_vertex(),
                    shader.describe_opaque_fragment(),
                    options,
                )
                .with_depth_write()
                .describe_render_pipeline();
                descriptor.primitive.cull_mode = Some(wgpu::Face::Back);
                descriptor.initialize_with_prefix_layouts(
                    device,
                    &[projection_resources.bind_group_layout()],
                )
            });
        }
        if has_blended_bg {
            globe_bg.blended.get_or_insert_with(|| {
                let mut descriptor = TilePipeline::new(
                    "globe_background_pipeline".into(),
                    *settings,
                    shader.describe_vertex(),
                    shader.describe_fragment(),
                    options,
                )
                .with_depth_write()
                .describe_render_pipeline();
                descriptor.primitive.cull_mode = Some(wgpu::Face::Back);
                descriptor.initialize_with_prefix_layouts(
                    device,
                    &[projection_resources.bind_group_layout()],
                )
            });
        }

        if needs_atmosphere {
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
                // The scattering lies behind the globe, its terrain and everything else that writes
                // depth, as GL JS draws it with a read-only depth test.
                .with_depth_test()
                .describe_render_pipeline()
                .initialize_with_prefix_layouts(
                    device,
                    &[projection_resources.bind_group_layout()],
                );
                AtmosphereRenderPipeline(pipeline)
            });
        }
    }

    if needs_sky {
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
    }

    Ok(())
}

/// Pipelines drawing a flat fullscreen background (`solid` omits fragment `discard`).
#[derive(Default)]
pub struct BackgroundRenderPipeline {
    /// Horizon/world-edge-clipped opaque (`blend: None`) background pipeline.
    pub clipped_opaque: Option<wgpu::RenderPipeline>,
    /// Horizon/world-edge-clipped background pipeline for tilted or partial flat views.
    pub clipped: Option<wgpu::RenderPipeline>,
    /// Zero-`discard` opaque (`blend: None`) background pipeline when the flat world fills viewport.
    pub solid_opaque: Option<wgpu::RenderPipeline>,
    /// Zero-`discard` background pipeline when the flat world covers the entire viewport.
    pub solid: Option<wgpu::RenderPipeline>,
}

/// Pipelines drawing background paint on a globe mesh (`opaque` skips color blending).
#[derive(Default)]
pub struct GlobeBackgroundRenderPipeline {
    /// Opaque (`blend: None`) globe background pipeline for alpha `1.0` backgrounds.
    pub opaque: Option<wgpu::RenderPipeline>,
    /// Premultiplied-alpha blended globe background pipeline.
    pub blended: Option<wgpu::RenderPipeline>,
}

/// Pipeline drawing optional atmospheric scattering.
pub struct AtmosphereRenderPipeline(pub wgpu::RenderPipeline);

/// Pipeline filling the screen above the horizon with the sky.
pub struct SkyRenderPipeline(pub wgpu::RenderPipeline);
