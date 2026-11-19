//! Style-scoped GPU render pipeline construction helpers for [`resource_system`](super::resource_system).

use crate::{
    render::{
        eventually::Eventually,
        resource::{RenderPipeline, TilePipeline},
        shaders::{self, Shader},
        STENCIL_MARK,
    },
    vector::{CirclePipeline, ExtrusionPipeline, LinePipeline, VectorPipeline},
};

pub(super) struct PipelineSetup<'a> {
    pub(super) device: &'a wgpu::Device,
    pub(super) settings: crate::render::settings::RendererSettings,
    pub(super) format: wgpu::TextureFormat,
    pub(super) multisampling: bool,
    pub(super) projection: &'a wgpu::BindGroupLayout,
    /// The second bind group of every pipeline that draws an image.
    pub(super) pattern: wgpu::BindGroupLayout,
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

    pub(super) fn create_pattern(&self) -> crate::render::resource::FeaturePipelinePair {
        self.create(
            "fill_pattern_pipeline",
            &shaders::FillPatternShader {
                format: self.format,
            },
            &[self.projection, &self.pattern],
            false,
        )
    }

    fn extrusion_descriptor(
        &self,
        pass: shaders::ExtrusionPass,
        vertex: crate::render::resource::VertexState,
        fragment: crate::render::resource::FragmentState,
    ) -> crate::render::resource::RenderPipelineDescriptor {
        use shaders::ExtrusionPass::{
            Clear, Color, Depth, PatternColor, TranslucentBack, TranslucentFront,
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
            vertex,
            fragment,
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
        descriptor
    }

    fn create_extrusion(
        &self,
        pass: shaders::ExtrusionPass,
    ) -> crate::render::resource::FeaturePipelinePair {
        let shader = shaders::FillExtrusionShader {
            format: self.format,
            pass,
        };
        let descriptor =
            self.extrusion_descriptor(pass, shader.describe_vertex(), shader.describe_fragment());
        let layouts: &[&wgpu::BindGroupLayout] = if pass == shaders::ExtrusionPass::PatternColor {
            &[self.projection, &self.pattern]
        } else {
            &[self.projection]
        };
        descriptor.initialize_pair_with_binding(self.device, layouts, Self::layer_color_binding())
    }

    fn create_compact_extrusion(&self, pass: shaders::ExtrusionPass) -> wgpu::RenderPipeline {
        let shader = shaders::FillExtrusionCompactShader {
            format: self.format,
            pass,
        };
        let mut descriptor =
            self.extrusion_descriptor(pass, shader.describe_vertex(), shader.describe_fragment());
        if let Some(label) = descriptor.label.take() {
            descriptor.label = Some(std::borrow::Cow::Owned(format!("{label}_compact")));
        }
        descriptor.initialize_with_prefix_layouts(self.device, &[self.projection])
    }

    pub(super) fn initialize_extrusion(
        &self,
        extrusion: &mut Eventually<ExtrusionPipeline>,
        needs_pattern: bool,
        needs_translucent: bool,
    ) {
        let ep = extrusion.initialize(|| ExtrusionPipeline {
            depth: self.create_extrusion(shaders::ExtrusionPass::Depth),
            color: self.create_extrusion(shaders::ExtrusionPass::Color),
            clear: self.create_extrusion(shaders::ExtrusionPass::Clear),
            compact_depth: self.create_compact_extrusion(shaders::ExtrusionPass::Depth),
            compact_color: self.create_compact_extrusion(shaders::ExtrusionPass::Color),
            compact_clear: self.create_compact_extrusion(shaders::ExtrusionPass::Clear),
            pattern: None,
            translucent_back: None,
            translucent_front: None,
            compact_translucent_back: None,
            compact_translucent_front: None,
        });
        if needs_pattern {
            ep.pattern
                .get_or_insert_with(|| self.create_extrusion(shaders::ExtrusionPass::PatternColor));
        }
        if needs_translucent {
            ep.translucent_back.get_or_insert_with(|| {
                self.create_extrusion(shaders::ExtrusionPass::TranslucentBack)
            });
            ep.translucent_front.get_or_insert_with(|| {
                self.create_extrusion(shaders::ExtrusionPass::TranslucentFront)
            });
            ep.compact_translucent_back.get_or_insert_with(|| {
                self.create_compact_extrusion(shaders::ExtrusionPass::TranslucentBack)
            });
            ep.compact_translucent_front.get_or_insert_with(|| {
                self.create_compact_extrusion(shaders::ExtrusionPass::TranslucentFront)
            });
        }
    }

    pub(super) fn initialize(
        &self,
        vector: &mut Eventually<VectorPipeline>,
        line: &mut Eventually<LinePipeline>,
        circle: &mut Eventually<CirclePipeline>,
        dashes: Option<&wgpu::BindGroupLayout>,
        needs: VectorPipelineNeeds,
    ) {
        if needs.fill {
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
        }
        if needs.flat_lines || needs.terrain_lines || needs.elevated_lines {
            if let Some(dashes) = dashes {
                let line_shader = shaders::LineShader {
                    format: self.format,
                };
                let line = line.initialize(|| LinePipeline {
                    flat: None,
                    structure: None,
                    elevated: None,
                    terrain: None,
                });
                if needs.flat_lines {
                    line.flat.get_or_insert_with(|| {
                        self.create(
                            "line_pipeline",
                            &line_shader,
                            &[self.projection, dashes],
                            false,
                        )
                    });
                }
                if needs.terrain_lines {
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
                if needs.elevated_lines {
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
            }
        }
        if needs.circle {
            let cp = circle.initialize(|| {
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
                    feature_stroke: None,
                    dynamic_radius: None,
                }
            });
            if needs.circle_feature_stroke {
                cp.feature_stroke.get_or_insert_with(|| {
                    self.describe(
                        "circle_feature_stroke_pipeline",
                        &shaders::CircleFeatureStrokeShader {
                            format: self.format,
                        },
                        false,
                        false,
                    )
                    .initialize_with_prefix_layouts(self.device, &[self.projection])
                });
            }
            if needs.circle_dynamic_radius {
                cp.dynamic_radius.get_or_insert_with(|| {
                    self.describe(
                        "circle_dynamic_radius_pipeline",
                        &shaders::CircleDynamicRadiusShader {
                            format: self.format,
                        },
                        false,
                        false,
                    )
                    .initialize_with_prefix_layouts(self.device, &[self.projection])
                });
            }
        }
    }
}

pub(super) struct VectorPipelineNeeds {
    pub(super) fill: bool,
    pub(super) flat_lines: bool,
    pub(super) terrain_lines: bool,
    pub(super) elevated_lines: bool,
    pub(super) circle: bool,
    pub(super) circle_feature_stroke: bool,
    pub(super) circle_dynamic_radius: bool,
}
