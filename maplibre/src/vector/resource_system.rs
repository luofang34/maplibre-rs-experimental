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
        resource::BufferPool, CirclePipeline, ExtrusionPipeline, LinePipeline, VectorBufferPool,
        VectorPipeline,
    },
};

/// The stencil bit no tile reference uses.
const STENCIL_MARK: u32 = 0x80;

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
    let Some(dashes_layout) = world
        .resources
        .get::<super::line_dash::LineDashResources>()
        .map(|dashes| dashes.layout.clone())
    else {
        return Err(SystemError::Dependencies);
    };
    let missing_pattern_pipeline = world
        .resources
        .get::<super::pattern::PatternResources>()
        .is_none_or(|patterns| patterns.pipeline().is_none());
    let Some((
        buffer_pool,
        vector_pipeline,
        line_pipeline,
        circle_pipeline,
        extrusion_pipeline,
        Initialized(projection_resources),
    )) = world.resources.query_mut::<(
        &mut Eventually<VectorBufferPool>,
        &mut Eventually<VectorPipeline>,
        &mut Eventually<LinePipeline>,
        &mut Eventually<CirclePipeline>,
        &mut Eventually<ExtrusionPipeline>,
        &mut Eventually<ProjectionGpuResources>,
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
        pattern: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("pattern layout"),
            entries: &super::pattern::layout_entries(),
        }),
    };
    setup.initialize(
        vector_pipeline,
        line_pipeline,
        circle_pipeline,
        &dashes_layout,
    );
    setup.initialize_extrusion(extrusion_pipeline);
    let pattern_pipeline = missing_pattern_pipeline.then(|| setup.create_pattern());
    let pattern_layout = setup.pattern.clone();
    if world
        .resources
        .get::<super::pattern::PatternResources>()
        .is_none()
    {
        world
            .resources
            .insert(super::pattern::PatternResources::new(
                device,
                pattern_layout,
            ));
    }
    if let (Some(pipeline), Some(patterns)) = (
        pattern_pipeline,
        world
            .resources
            .get_mut::<super::pattern::PatternResources>(),
    ) {
        patterns.set_pipeline(pipeline);
    }
    if let Some(patterns) = world
        .resources
        .get_mut::<super::pattern::PatternResources>()
    {
        patterns.update(device, queue, style, view_state.style_zoom().value());
    }
    Ok(())
}

struct PipelineSetup<'a> {
    device: &'a wgpu::Device,
    settings: crate::render::settings::RendererSettings,
    format: wgpu::TextureFormat,
    multisampling: bool,
    projection: &'a wgpu::BindGroupLayout,
    /// The second bind group of every pipeline that draws an image.
    pattern: wgpu::BindGroupLayout,
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
            crate::render::resource::TilePipelineOptions {
                depth_stencil_enabled: true,
                update_stencil: false,
                debug_stencil: false,
                wireframe: false,
                multisampling: self.multisampling,
                textured: false,
            },
        );
        let pipeline = if depth {
            pipeline.with_depth_write()
        } else {
            pipeline
        };
        let mut descriptor = pipeline.describe_render_pipeline();
        if depth {
            // Casing and deck are paint layers on the same road surface. Test against
            // terrain, but let style order composite them without self-occluding edges.
            if let Some(state) = &mut descriptor.depth_stencil {
                state.depth_write_enabled = Some(false);
            }
        }
        descriptor.initialize_with_prefix_layouts(self.device, layouts)
    }

    fn create_pattern(&self) -> wgpu::RenderPipeline {
        self.create(
            "fill_pattern_pipeline",
            &shaders::FillPatternShader {
                format: self.format,
            },
            &[self.projection, &self.pattern],
            false,
        )
    }

    fn create_extrusion(&self, pass: shaders::ExtrusionPass) -> wgpu::RenderPipeline {
        use shaders::ExtrusionPass::{Clear, Color, Depth, PatternColor};

        let shader = shaders::FillExtrusionShader {
            format: self.format,
            pass,
        };
        let mut descriptor = TilePipeline::new(
            match pass {
                Depth => "extrusion_depth_pipeline",
                Color => "extrusion_color_pipeline",
                PatternColor => "extrusion_pattern_pipeline",
                Clear => "extrusion_clear_pipeline",
            }
            .into(),
            self.settings,
            shader.describe_vertex(),
            shader.describe_fragment(),
            crate::render::resource::TilePipelineOptions {
                depth_stencil_enabled: true,
                update_stencil: false,
                debug_stencil: true,
                wireframe: false,
                multisampling: self.multisampling,
                textured: false,
            },
        )
        .with_depth_write()
        .describe_render_pipeline();
        if let Some(state) = &mut descriptor.depth_stencil {
            state.depth_write_enabled = Some(pass == Depth);
            state.depth_compare = Some(match pass {
                Depth | Color | PatternColor => wgpu::CompareFunction::GreaterEqual,
                Clear => wgpu::CompareFunction::Always,
            });
            // Tile references use the low seven bits; the top bit marks the pixels the colour
            // pass has drawn, so a surface that two tiles both hold is blended once.
            let marked = wgpu::StencilFaceState {
                compare: wgpu::CompareFunction::Equal,
                fail_op: wgpu::StencilOperation::Keep,
                depth_fail_op: wgpu::StencilOperation::Keep,
                pass_op: wgpu::StencilOperation::Invert,
            };
            let unmark = wgpu::StencilFaceState {
                compare: wgpu::CompareFunction::Always,
                fail_op: wgpu::StencilOperation::Keep,
                depth_fail_op: wgpu::StencilOperation::Keep,
                pass_op: wgpu::StencilOperation::Zero,
            };
            match pass {
                Depth => {}
                Color | PatternColor => state.stencil.front = marked,
                Clear => state.stencil.front = unmark,
            }
            state.stencil.back = state.stencil.front;
            if pass != Depth {
                state.stencil.read_mask = STENCIL_MARK;
                state.stencil.write_mask = STENCIL_MARK;
            }
        }
        let layouts: &[&wgpu::BindGroupLayout] = if pass == PatternColor {
            &[self.projection, &self.pattern]
        } else {
            &[self.projection]
        };
        descriptor.initialize_with_prefix_layouts(self.device, layouts)
    }

    fn initialize_extrusion(&self, extrusion: &mut Eventually<ExtrusionPipeline>) {
        extrusion.initialize(|| ExtrusionPipeline {
            depth: self.create_extrusion(shaders::ExtrusionPass::Depth),
            color: self.create_extrusion(shaders::ExtrusionPass::Color),
            clear: self.create_extrusion(shaders::ExtrusionPass::Clear),
            pattern: self.create_extrusion(shaders::ExtrusionPass::PatternColor),
        });
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
