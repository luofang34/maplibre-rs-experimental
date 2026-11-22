//! Style-scoped GPU render pipeline construction helpers for [`resource_system`](super::resource_system).

use crate::{
    render::{
        eventually::Eventually,
        resource::{RenderPipeline, TilePipeline},
        shaders::{self, Shader},
    },
    vector::{CirclePipeline, LinePipeline, VectorPipeline},
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
    pub(super) fn layer_color_binding() -> crate::render::resource::UniformFeatureBinding {
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

    fn create_fill(
        &self,
        name: &'static str,
        globe: bool,
        opaque: bool,
    ) -> crate::render::resource::FeaturePipelinePair {
        let shader = shaders::FillShader {
            format: self.format,
        };
        let pipeline = TilePipeline::new(
            name.into(),
            self.settings,
            shader.describe_vertex_for_globe(globe),
            if opaque {
                shader.describe_fragment_opaque()
            } else {
                shader.describe_fragment()
            },
            crate::render::resource::TilePipelineOptions {
                depth_stencil_enabled: true,
                update_stencil: false,
                debug_stencil: false,
                wireframe: false,
                multisampling: self.multisampling,
                textured: false,
            },
        );
        pipeline
            .describe_render_pipeline()
            .initialize_pair_with_binding(
                self.device,
                &[self.projection],
                Self::layer_color_binding(),
            )
    }

    pub(super) fn initialize(
        &self,
        vector: &mut Eventually<VectorPipeline>,
        line: &mut Eventually<LinePipeline>,
        circle: &mut Eventually<CirclePipeline>,
        dashes: Option<&wgpu::BindGroupLayout>,
        needs: VectorPipelineNeeds,
    ) {
        self.init_fill_pipelines(vector, &needs);
        self.init_line_pipelines(line, dashes, &needs);
        self.init_circle_pipelines(circle, &needs);
    }

    fn init_fill_pipelines(
        &self,
        vector: &mut Eventually<VectorPipeline>,
        needs: &VectorPipelineNeeds,
    ) {
        if !(needs.flat_fill_opaque
            || needs.flat_fill_blended
            || needs.globe_fill_opaque
            || needs.globe_fill_blended)
        {
            return;
        }
        let vp = vector.initialize(VectorPipeline::default);
        if needs.flat_fill_opaque {
            vp.flat_opaque
                .get_or_insert_with(|| self.create_fill("vector_flat_opaque", false, true));
        }
        if needs.flat_fill_blended {
            vp.flat_blended
                .get_or_insert_with(|| self.create_fill("vector_flat_blended", false, false));
        }
        if needs.globe_fill_opaque {
            vp.globe_opaque
                .get_or_insert_with(|| self.create_fill("vector_globe_opaque", true, true));
        }
        if needs.globe_fill_blended {
            vp.globe_blended
                .get_or_insert_with(|| self.create_fill("vector_globe_blended", true, false));
        }
    }

    fn init_line_pipelines(
        &self,
        line: &mut Eventually<LinePipeline>,
        dashes: Option<&wgpu::BindGroupLayout>,
        needs: &VectorPipelineNeeds,
    ) {
        if !(needs.plain_flat_lines
            || needs.plain_globe_lines
            || needs.flat_lines
            || needs.terrain_lines
            || needs.elevated_lines)
        {
            return;
        }
        let Some(dashes) = dashes else {
            return;
        };
        let line = line.initialize(|| LinePipeline {
            flat_plain: None,
            globe_plain: None,
            flat: None,
            structure: None,
            elevated: None,
            terrain: None,
        });
        self.init_plain_line_pipelines(line, needs);
        self.init_styled_line_pipelines(line, dashes, needs);
    }

    fn init_plain_line_pipelines(&self, line: &mut LinePipeline, needs: &VectorPipelineNeeds) {
        if needs.plain_flat_lines {
            line.flat_plain.get_or_insert_with(|| {
                self.create(
                    "plain_line_flat_pipeline",
                    &shaders::PlainLineShader {
                        format: self.format,
                        globe: false,
                    },
                    &[self.projection],
                    false,
                )
            });
        }
        if needs.plain_globe_lines {
            line.globe_plain.get_or_insert_with(|| {
                self.create(
                    "plain_line_globe_pipeline",
                    &shaders::PlainLineShader {
                        format: self.format,
                        globe: true,
                    },
                    &[self.projection],
                    false,
                )
            });
        }
    }

    fn init_styled_line_pipelines(
        &self,
        line: &mut LinePipeline,
        dashes: &wgpu::BindGroupLayout,
        needs: &VectorPipelineNeeds,
    ) {
        let line_shader = shaders::LineShader {
            format: self.format,
        };
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

    fn init_circle_pipelines(
        &self,
        circle: &mut Eventually<CirclePipeline>,
        needs: &VectorPipelineNeeds,
    ) {
        if !needs.circle {
            return;
        }
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

pub(super) struct VectorPipelineNeeds {
    pub(super) flat_fill_opaque: bool,
    pub(super) flat_fill_blended: bool,
    pub(super) globe_fill_opaque: bool,
    pub(super) globe_fill_blended: bool,
    pub(super) plain_flat_lines: bool,
    pub(super) plain_globe_lines: bool,
    pub(super) flat_lines: bool,
    pub(super) terrain_lines: bool,
    pub(super) elevated_lines: bool,
    pub(super) circle: bool,
    pub(super) circle_feature_stroke: bool,
    pub(super) circle_dynamic_radius: bool,
}
