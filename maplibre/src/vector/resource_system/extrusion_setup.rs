//! Extrusion pipeline setup helpers extracted from `setup.rs`.

use super::setup::PipelineSetup;
use crate::{
    render::{
        eventually::Eventually,
        resource::{RenderPipeline, TilePipeline},
        shaders, STENCIL_MARK,
    },
    vector::{ExtrusionModePipelines, ExtrusionPipeline},
};

#[derive(Clone, Copy)]
pub(super) struct ExtrusionPipelineNeeds {
    pub(super) globe: bool,
    pub(super) standard_opaque: bool,
    pub(super) compact_opaque: bool,
    pub(super) pattern: bool,
    pub(super) standard_translucent: bool,
    pub(super) compact_translucent: bool,
    pub(super) clear_mark: bool,
}

impl PipelineSetup<'_> {
    fn extrusion_descriptor(
        &self,
        pass: shaders::ExtrusionPass,
        vertex: crate::render::resource::VertexState,
        fragment: crate::render::resource::FragmentState,
    ) -> crate::render::resource::RenderPipelineDescriptor {
        use shaders::ExtrusionPass::{
            Color, Depth, PatternColor, TranslucentBack, TranslucentFront,
        };

        let mut descriptor = TilePipeline::new(
            match pass {
                Depth => "extrusion_depth_pipeline",
                Color => "extrusion_color_pipeline",
                PatternColor => "extrusion_pattern_pipeline",
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
            state.depth_compare = Some(wgpu::CompareFunction::GreaterEqual);
            // Tile references use the low seven bits; the top bit marks the pixels the colour
            // pass has drawn, so a surface that two tiles both hold is blended once.
            let marked = wgpu::StencilFaceState {
                compare: wgpu::CompareFunction::Equal,
                fail_op: wgpu::StencilOperation::Keep,
                depth_fail_op: wgpu::StencilOperation::Keep,
                pass_op: wgpu::StencilOperation::Invert,
            };
            match pass {
                Depth | TranslucentBack | TranslucentFront => {}
                Color | PatternColor => state.stencil.front = marked,
            }
            state.stencil.back = state.stencil.front;
            if matches!(pass, Color | PatternColor) {
                state.stencil.read_mask = STENCIL_MARK;
                state.stencil.write_mask = STENCIL_MARK;
            } else if matches!(pass, TranslucentBack | TranslucentFront) {
                state.stencil.read_mask = 0;
                state.stencil.write_mask = 0;
            }
        }
        descriptor
    }

    fn create_extrusion_depth(
        &self,
        pass: shaders::ExtrusionPass,
        globe: bool,
    ) -> wgpu::RenderPipeline {
        let shader = shaders::FillExtrusionShader {
            format: self.format,
            pass,
        };
        let descriptor = self.extrusion_descriptor(
            pass,
            shader.describe_vertex_for_globe(globe),
            shader.describe_fragment_for_globe(globe),
        );
        descriptor.initialize_with_prefix_layouts(self.device, &[self.projection])
    }

    fn create_extrusion_clear_mark(&self) -> wgpu::RenderPipeline {
        use shaders::Shader;
        let shader = shaders::StencilClearShader {
            format: self.format,
        };
        let mut descriptor = TilePipeline::new(
            "extrusion_clear_mark_pipeline".into(),
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
        descriptor.primitive.cull_mode = None;
        if let Some(state) = &mut descriptor.depth_stencil {
            state.depth_write_enabled = Some(false);
            state.depth_compare = Some(wgpu::CompareFunction::Always);
            let unmark = wgpu::StencilFaceState {
                compare: wgpu::CompareFunction::Always,
                fail_op: wgpu::StencilOperation::Keep,
                depth_fail_op: wgpu::StencilOperation::Keep,
                pass_op: wgpu::StencilOperation::Zero,
            };
            state.stencil.front = unmark;
            state.stencil.back = unmark;
            state.stencil.read_mask = STENCIL_MARK;
            state.stencil.write_mask = STENCIL_MARK;
        }
        descriptor.initialize_with_prefix_layouts(self.device, &[])
    }

    fn create_extrusion(
        &self,
        pass: shaders::ExtrusionPass,
        globe: bool,
    ) -> crate::render::resource::FeaturePipelinePair {
        let shader = shaders::FillExtrusionShader {
            format: self.format,
            pass,
        };
        let descriptor = self.extrusion_descriptor(
            pass,
            shader.describe_vertex_for_globe(globe),
            shader.describe_fragment_for_globe(globe),
        );
        let layouts: &[&wgpu::BindGroupLayout] = if pass == shaders::ExtrusionPass::PatternColor {
            &[self.projection, &self.pattern]
        } else {
            &[self.projection]
        };
        descriptor.initialize_pair_with_binding(self.device, layouts, Self::layer_color_binding())
    }

    fn create_compact_extrusion(
        &self,
        pass: shaders::ExtrusionPass,
        globe: bool,
    ) -> wgpu::RenderPipeline {
        let shader = shaders::FillExtrusionCompactShader {
            format: self.format,
            pass,
        };
        let mut descriptor = self.extrusion_descriptor(
            pass,
            shader.describe_vertex_for_globe(globe),
            shader.describe_fragment_for_globe(globe),
        );
        if let Some(label) = descriptor.label.take() {
            descriptor.label = Some(std::borrow::Cow::Owned(format!("{label}_compact")));
        }
        descriptor.initialize_with_prefix_layouts(self.device, &[self.projection])
    }

    pub(super) fn initialize_extrusion(
        &self,
        extrusion: &mut Eventually<ExtrusionPipeline>,
        needs: ExtrusionPipelineNeeds,
    ) {
        let pipeline = extrusion.initialize(ExtrusionPipeline::default);
        if needs.clear_mark && pipeline.clear_mark.is_none() {
            pipeline.clear_mark = Some(self.create_extrusion_clear_mark());
        }
        if !(needs.standard_opaque
            || needs.compact_opaque
            || needs.pattern
            || needs.standard_translucent
            || needs.compact_translucent)
        {
            return;
        }
        let globe = needs.globe;
        let ep = pipeline
            .slot_mut(globe)
            .get_or_insert_with(ExtrusionModePipelines::default);
        if needs.standard_opaque {
            ep.depth.get_or_insert_with(|| {
                self.create_extrusion_depth(shaders::ExtrusionPass::Depth, globe)
            });
            ep.color
                .get_or_insert_with(|| self.create_extrusion(shaders::ExtrusionPass::Color, globe));
        }
        if needs.compact_opaque {
            ep.compact_depth.get_or_insert_with(|| {
                self.create_compact_extrusion(shaders::ExtrusionPass::Depth, globe)
            });
            ep.compact_color.get_or_insert_with(|| {
                self.create_compact_extrusion(shaders::ExtrusionPass::Color, globe)
            });
        }
        if needs.pattern {
            ep.pattern.get_or_insert_with(|| {
                self.create_extrusion(shaders::ExtrusionPass::PatternColor, globe)
            });
        }
        if needs.standard_translucent {
            ep.translucent_back.get_or_insert_with(|| {
                self.create_extrusion(shaders::ExtrusionPass::TranslucentBack, globe)
            });
            ep.translucent_front.get_or_insert_with(|| {
                self.create_extrusion(shaders::ExtrusionPass::TranslucentFront, globe)
            });
        }
        if needs.compact_translucent {
            ep.compact_translucent_back.get_or_insert_with(|| {
                self.create_compact_extrusion(shaders::ExtrusionPass::TranslucentBack, globe)
            });
            ep.compact_translucent_front.get_or_insert_with(|| {
                self.create_compact_extrusion(shaders::ExtrusionPass::TranslucentFront, globe)
            });
        }
    }
}
