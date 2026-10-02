//! Triangle-list pipeline state for tile masking, textured layers and reversed-Z geometry.

use std::borrow::Cow;

use crate::render::{
    resource::{FragmentState, RenderPipeline, RenderPipelineDescriptor, VertexState},
    settings::RendererSettings,
};

/// Fixed GPU state shared by all draw calls in a tile pipeline.
#[derive(Clone, Copy, Default)]
pub struct TilePipelineOptions {
    /// Enables depth and stencil attachment tests.
    pub depth_stencil_enabled: bool,
    /// Replaces stencil values when a fragment passes.
    pub update_stencil: bool,
    /// Draws without restricting fragments to the tile stencil.
    pub debug_stencil: bool,
    /// Draws triangle edges.
    pub wireframe: bool,
    /// Uses the renderer's multisample count.
    pub multisampling: bool,
    /// Binds a sampled texture and a filtering sampler.
    pub textured: bool,
}

/// Tile draw state with optional texture bindings, stencil masking and depth writes.
/// Color blending and vertex interpretation come from the supplied shader-stage descriptions.
pub struct TilePipeline {
    name: Cow<'static, str>,
    options: TilePipelineOptions,
    /// Writes and tests depth with the reversed-Z convention instead of painter's order.
    depth_write: bool,
    settings: RendererSettings,

    vertex_state: VertexState,
    fragment_state: FragmentState,
}

impl TilePipeline {
    /// Describes a tile pipeline without compiling shaders or allocating GPU resources.
    /// When multisampling is enabled in `options`, the caller must check that the settings'
    /// sample count is supported by the color and depth formats. Depth writes start disabled.
    pub fn new(
        name: Cow<'static, str>,
        settings: RendererSettings,
        vertex_state: VertexState,
        fragment_state: FragmentState,
        options: TilePipelineOptions,
    ) -> Self {
        TilePipeline {
            name,
            options,
            depth_write: false,
            settings,
            vertex_state,
            fragment_state,
        }
    }

    /// Enables depth writes for 3D geometry such as terrain; painter's order no longer applies.
    pub fn with_depth_write(mut self) -> Self {
        self.depth_write = true;
        self
    }
}

impl RenderPipeline for TilePipeline {
    fn describe_render_pipeline(self) -> RenderPipelineDescriptor {
        let layout = self.texture_layout();
        let depth_stencil = self.depth_stencil();

        RenderPipelineDescriptor {
            label: Some(self.name),
            layout,
            vertex: self.vertex_state,
            fragment: if self.settings.overdraw_inspector {
                counting_overdraw(self.fragment_state)
            } else {
                self.fragment_state
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                polygon_mode: if self.options.update_stencil {
                    wgpu::PolygonMode::Fill
                } else if self.options.wireframe {
                    wgpu::PolygonMode::Line
                } else {
                    wgpu::PolygonMode::Fill
                },
                front_face: wgpu::FrontFace::Ccw,
                strip_index_format: None,
                cull_mode: None, // Maps look the same from he bottom and above -> No culling needed
                conservative: false,
                unclipped_depth: false,
            },
            depth_stencil,
            multisample: wgpu::MultisampleState {
                count: if self.options.multisampling {
                    self.settings.msaa.samples
                } else {
                    1
                },
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
        }
    }
}

/// One step of the overdraw inspector: a fragment adds an eighth of white, as GL JS adds one of
/// eight steps per draw.
const OVERDRAW_FRAGMENT: &str = "@fragment
fn main() -> @location(0) vec4<f32> {
    return vec4<f32>(0.125, 0.125, 0.125, 1.0);
}
";

/// A fragment stage that counts draws instead of shading; stages that write no colour, or more
/// than one target, keep their own.
fn counting_overdraw(fragment: FragmentState) -> FragmentState {
    let writes_one_colour = matches!(
        fragment.targets.as_slice(),
        [Some(target)] if !target.write_mask.is_empty()
    );
    if !writes_one_colour {
        return fragment;
    }
    let adding = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    };
    FragmentState {
        source: OVERDRAW_FRAGMENT,
        entry_point: "main",
        targets: fragment
            .targets
            .into_iter()
            .map(|target| {
                target.map(|target| wgpu::ColorTargetState {
                    blend: Some(wgpu::BlendState {
                        color: adding,
                        alpha: adding,
                    }),
                    ..target
                })
            })
            .collect(),
    }
}

impl TilePipeline {
    fn stencil_face(&self) -> wgpu::StencilFaceState {
        if self.options.update_stencil {
            wgpu::StencilFaceState {
                compare: wgpu::CompareFunction::Always, // Allow ALL values to update the stencil
                fail_op: wgpu::StencilOperation::Keep,
                depth_fail_op: wgpu::StencilOperation::Keep, // This is used when the depth test already failed
                pass_op: wgpu::StencilOperation::Replace,
            }
        } else {
            wgpu::StencilFaceState {
                compare: if self.options.debug_stencil {
                    wgpu::CompareFunction::Always
                } else {
                    wgpu::CompareFunction::Equal
                },
                fail_op: wgpu::StencilOperation::Keep,
                depth_fail_op: wgpu::StencilOperation::Keep,
                pass_op: wgpu::StencilOperation::Keep,
            }
        }
    }
    fn texture_layout(&self) -> Option<Vec<Vec<wgpu::BindGroupLayoutEntry>>> {
        if self.options.textured {
            Some(vec![vec![
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
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ]])
        } else {
            None
        }
    }
    fn depth_stencil(&self) -> Option<wgpu::DepthStencilState> {
        let stencil_state = self.stencil_face();
        if !self.options.depth_stencil_enabled {
            None
        } else {
            Some(wgpu::DepthStencilState {
                format: self.settings.depth_texture_format,
                // Layers use painter's algorithm (draw order), matching MapLibre GL
                // behavior, and stencil handles tile masking. Only 3D geometry writes
                // depth, using reversed-Z where nearer fragments have greater depth.
                depth_write_enabled: Some(self.depth_write),
                depth_compare: Some(if self.depth_write {
                    wgpu::CompareFunction::GreaterEqual
                } else {
                    wgpu::CompareFunction::Always
                }),
                stencil: wgpu::StencilState {
                    front: stencil_state,
                    back: stencil_state,
                    read_mask: 0xff, // Applied to stencil values being read from the stencil buffer
                    write_mask: 0xff, // Applied to fragment stencil values before being written to  the stencil buffer
                },
                bias: wgpu::DepthBiasState::default(),
            })
        }
    }
}

#[cfg(test)]
mod tests;
