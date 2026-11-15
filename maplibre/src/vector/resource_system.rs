//! Prepares GPU-owned resources by initializing them if they are uninitialized or out-of-date.
use crate::{
    context::MapContext,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        resource::{RenderPipeline, TilePipeline},
        shaders,
        shaders::Shader,
        RenderResources, Renderer, STENCIL_MARK,
    },
    tcs::system::{SystemError, SystemResult},
    vector::{
        resource::BufferPool, CirclePipeline, ExtrusionPipeline, LinePipeline, VectorBufferPool,
        VectorPipeline,
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
    let terrain_active = world
        .resources
        .get::<crate::terrain::TerrainFrame>()
        .is_some_and(|frame| frame.active);
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

    buffer_pool.initialize(|| {
        BufferPool::from_device_with_sizes(device, settings.buffer_pools)
            .with_decoupled_feature_metadata()
            .with_shared_layer_metadata()
    });

    let setup = PipelineSetup {
        device,
        settings: *settings,
        format: surface.surface_format(),
        multisampling: surface.is_multisampling_supported(settings.msaa),
        projection: projection_resources.bind_group_layout(),
        pattern: crate::render::resource::create_bind_group_layout_cached(
            device,
            Some("pattern layout"),
            &super::pattern::layout_entries(),
        ),
    };
    let needs_terrain_lines =
        !cfg!(target_arch = "wasm32") || style.terrain.is_some() || terrain_active;
    let needs_elevated_lines = !cfg!(target_arch = "wasm32")
        || style.layers.iter().any(|layer| {
            matches!(
                super::structures::kind(layer),
                Some(super::structures::StructureKind::ElevatedLine)
            )
        });
    let needs_circle =
        !cfg!(target_arch = "wasm32") || style.layers.iter().any(|layer| layer.type_ == "circle");
    setup.initialize(
        vector_pipeline,
        line_pipeline,
        circle_pipeline,
        &dashes_layout,
        (needs_terrain_lines, needs_elevated_lines, needs_circle),
    );
    let style_zoom = view_state.style_zoom().value();
    let needs_extrusion = !cfg!(target_arch = "wasm32")
        || style
            .layers
            .iter()
            .any(|layer| layer.type_ == "fill-extrusion");
    if needs_extrusion {
        setup.initialize_extrusion(extrusion_pipeline);
    }
    let needs_pattern = !cfg!(target_arch = "wasm32")
        || style.layers.iter().any(|layer| {
            layer.paint.as_ref().is_some_and(|paint| {
                super::pattern::pattern_name(paint, style_zoom).is_some()
                    || super::pattern::per_feature_pattern(paint).is_some()
            })
        });
    let pattern_pipeline =
        (missing_pattern_pipeline && needs_pattern).then(|| setup.create_pattern());
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
    fn layer_color_binding() -> crate::render::resource::UniformFeatureBinding {
        crate::render::resource::UniformFeatureBinding::LayerMetadata {
            layer_slot: 2,
            feature_slot: 3,
            attribute: wgpu::VertexAttribute {
                offset: std::mem::offset_of!(shaders::ShaderLayerMetadata, uniform_color) as u64,
                format: wgpu::VertexFormat::Float32x4,
                shader_location: 8,
            },
        }
    }

    fn create(
        &self,
        name: &'static str,
        shader: &impl Shader,
        layouts: &[&wgpu::BindGroupLayout],
        depth: bool,
    ) -> crate::render::resource::FeaturePipelinePair {
        self.create_with_clipping(name, shader, layouts, depth, true)
    }

    /// `clipped` keeps the pipeline inside its tile's stencil mask.
    fn create_with_clipping(
        &self,
        name: &'static str,
        shader: &impl Shader,
        layouts: &[&wgpu::BindGroupLayout],
        depth: bool,
        clipped: bool,
    ) -> crate::render::resource::FeaturePipelinePair {
        self.describe(name, shader, depth, clipped)
            .initialize_pair_with_binding(self.device, layouts, Self::layer_color_binding())
    }

    /// The pipeline of a line on the terrain surface: tested against the terrain depth as a
    /// structure is, with the terrain tile it lies on bound after the dash atlas. The stencil
    /// masks lie on the map plane, below a line lifted onto hills, so the line clips itself to
    /// its tiles instead.
    fn create_terrain_line(
        &self,
        dashes: &wgpu::BindGroupLayout,
    ) -> crate::render::resource::FeaturePipelinePair {
        let shader = shaders::TerrainLineShader {
            format: self.format,
        };
        let mut descriptor = self.describe("terrain_line_pipeline", &shader, true, false);
        descriptor.layout = Some(vec![
            crate::terrain::resources::TerrainResources::bind_group_layout_entries(),
        ]);
        descriptor.initialize_pair_with_binding(
            self.device,
            &[self.projection, dashes],
            Self::layer_color_binding(),
        )
    }

    fn describe(
        &self,
        name: &'static str,
        shader: &impl Shader,
        depth: bool,
        clipped: bool,
    ) -> crate::render::resource::RenderPipelineDescriptor {
        let pipeline = TilePipeline::new(
            name.into(),
            self.settings,
            shader.describe_vertex(),
            shader.describe_fragment(),
            crate::render::resource::TilePipelineOptions {
                depth_stencil_enabled: true,
                update_stencil: false,
                debug_stencil: !clipped,
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
        descriptor
    }

    fn create_pattern(&self) -> crate::render::resource::FeaturePipelinePair {
        self.create(
            "fill_pattern_pipeline",
            &shaders::FillPatternShader {
                format: self.format,
            },
            &[self.projection, &self.pattern],
            false,
        )
    }

    fn create_extrusion(
        &self,
        pass: shaders::ExtrusionPass,
    ) -> crate::render::resource::FeaturePipelinePair {
        use shaders::ExtrusionPass::{
            Clear, Color, Depth, PatternColor, TranslucentBack, TranslucentFront,
        };

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
                TranslucentBack => "extrusion_translucent_back_pipeline",
                TranslucentFront => "extrusion_translucent_front_pipeline",
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
        descriptor.primitive.cull_mode = match pass {
            TranslucentBack => Some(wgpu::Face::Front),
            TranslucentFront => Some(wgpu::Face::Back),
            _ => None,
        };
        if let Some(state) = &mut descriptor.depth_stencil {
            state.depth_write_enabled = Some(pass == Depth);
            state.depth_compare = Some(match pass {
                Depth | Color | PatternColor | TranslucentBack | TranslucentFront => {
                    wgpu::CompareFunction::GreaterEqual
                }
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
                Depth | TranslucentBack | TranslucentFront => {}
                Color | PatternColor => state.stencil.front = marked,
                Clear => state.stencil.front = unmark,
            }
            state.stencil.back = state.stencil.front;
            if matches!(pass, Color | PatternColor | Clear) {
                state.stencil.read_mask = STENCIL_MARK;
                state.stencil.write_mask = STENCIL_MARK;
            } else if matches!(pass, TranslucentBack | TranslucentFront) {
                state.stencil.read_mask = 0;
                state.stencil.write_mask = 0;
            }
        }
        let layouts: &[&wgpu::BindGroupLayout] = if pass == PatternColor {
            &[self.projection, &self.pattern]
        } else {
            &[self.projection]
        };
        descriptor.initialize_pair_with_binding(self.device, layouts, Self::layer_color_binding())
    }

    fn initialize_extrusion(&self, extrusion: &mut Eventually<ExtrusionPipeline>) {
        extrusion.initialize(|| ExtrusionPipeline {
            depth: self.create_extrusion(shaders::ExtrusionPass::Depth),
            color: self.create_extrusion(shaders::ExtrusionPass::Color),
            clear: self.create_extrusion(shaders::ExtrusionPass::Clear),
            pattern: self.create_extrusion(shaders::ExtrusionPass::PatternColor),
            translucent_back: self.create_extrusion(shaders::ExtrusionPass::TranslucentBack),
            translucent_front: self.create_extrusion(shaders::ExtrusionPass::TranslucentFront),
        });
    }

    fn initialize(
        &self,
        vector: &mut Eventually<VectorPipeline>,
        line: &mut Eventually<LinePipeline>,
        circle: &mut Eventually<CirclePipeline>,
        dashes: &wgpu::BindGroupLayout,
        (needs_terrain_lines, needs_elevated_lines, needs_circle): (bool, bool, bool),
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
        let line_shader = shaders::LineShader {
            format: self.format,
        };
        let line = line.initialize(|| LinePipeline {
            flat: self.create(
                "line_pipeline",
                &line_shader,
                &[self.projection, dashes],
                false,
            ),
            structure: None,
            elevated: None,
            terrain: None,
        });
        if needs_terrain_lines {
            line.structure.get_or_insert_with(|| {
                self.create(
                    "spatial_line_pipeline",
                    &line_shader,
                    &[self.projection, dashes],
                    true,
                )
            });
            line.terrain
                .get_or_insert_with(|| self.create_terrain_line(dashes));
        }
        if needs_elevated_lines {
            line.elevated.get_or_insert_with(|| {
                self.create_with_clipping(
                    "elevated_line_pipeline",
                    &line_shader,
                    &[self.projection, dashes],
                    true,
                    false,
                )
            });
        }
        if needs_circle {
            circle.initialize(|| {
                // Circles cross tile boundaries, as in GL JS, where each point belongs to one tile.
                CirclePipeline {
                    pair: self.create_with_clipping(
                        "circle_pipeline",
                        &shaders::CircleShader {
                            format: self.format,
                        },
                        &[self.projection],
                        false,
                        false,
                    ),
                    feature_stroke: self
                        .describe(
                            "circle_feature_stroke_pipeline",
                            &shaders::CircleFeatureStrokeShader {
                                format: self.format,
                            },
                            false,
                            false,
                        )
                        .initialize_with_prefix_layouts(self.device, &[self.projection]),
                    dynamic_radius: self
                        .describe(
                            "circle_dynamic_radius_pipeline",
                            &shaders::CircleDynamicRadiusShader {
                                format: self.format,
                            },
                            false,
                            false,
                        )
                        .initialize_with_prefix_layouts(self.device, &[self.projection]),
                }
            });
        }
    }
}
