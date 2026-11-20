use super::{attribute, FillShaderFeatureMetadata, GpuShaderVertex, Shader, ShaderTileMetadata};
use crate::render::resource::{FragmentState, VertexBufferLayout, VertexState};

/// The fill shader with the fill replaced by a repeating image.
pub struct FillPatternShader {
    /// Format of the color attachment used by the render pipeline.
    pub format: wgpu::TextureFormat,
}

impl Shader for FillPatternShader {
    fn describe_vertex(&self) -> VertexState {
        VertexState {
            source: concat!(
                include_str!(concat!(
                    env!("OUT_DIR"),
                    "/wgsl/render/shaders/projection.vertex.wgsl"
                )),
                include_str!(concat!(
                    env!("OUT_DIR"),
                    "/wgsl/render/shaders/fill_pattern.vertex.wgsl"
                ))
            ),
            ..FillShader {
                format: self.format,
            }
            .describe_vertex()
        }
    }

    fn describe_fragment(&self) -> FragmentState {
        FragmentState {
            source: include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/render/shaders/fill_pattern.fragment.wgsl"
            )),
            ..FillShader {
                format: self.format,
            }
            .describe_fragment()
        }
    }
}

/// Polygon-fill shader with premultiplied-alpha output.
pub struct FillShader {
    /// Format of the color attachment used by the render pipeline.
    pub format: wgpu::TextureFormat,
}

impl FillShader {
    /// Returns either the globe-aware vertex descriptor or the lean flat-Mercator variant.
    pub fn describe_vertex_for_globe(&self, globe: bool) -> VertexState {
        if globe {
            self.describe_vertex()
        } else {
            VertexState {
                source: include_str!(concat!(
                    env!("OUT_DIR"),
                    "/wgsl/render/shaders/fill_flat.vertex.wgsl"
                )),
                ..self.describe_vertex()
            }
        }
    }

    /// Returns the opaque fragment state (`blend: None`) for fills whose alpha is `1.0`, enabling
    /// Apple Silicon TBDR hardware hidden-surface removal.
    pub fn describe_fragment_opaque(&self) -> FragmentState {
        FragmentState {
            targets: vec![Some(wgpu::ColorTargetState {
                format: self.format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            ..self.describe_fragment()
        }
    }
}

impl Shader for FillShader {
    fn describe_vertex(&self) -> VertexState {
        VertexState {
            source: concat!(
                include_str!(concat!(
                    env!("OUT_DIR"),
                    "/wgsl/render/shaders/projection.vertex.wgsl"
                )),
                include_str!(concat!(
                    env!("OUT_DIR"),
                    "/wgsl/render/shaders/fill.vertex.wgsl"
                ))
            ),
            entry_point: "main",
            buffers: vec![
                // vertex data
                VertexBufferLayout {
                    array_stride: std::mem::size_of::<GpuShaderVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: vec![
                        attribute(0, wgpu::VertexFormat::Float32x2, 0),
                        attribute(
                            wgpu::VertexFormat::Float32x2.size(),
                            wgpu::VertexFormat::Float32x2,
                            1,
                        ),
                    ],
                },
                // tile metadata
                ShaderTileMetadata::basic_layout(),
                // layer metadata
                VertexBufferLayout {
                    array_stride: 0,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: vec![
                        attribute(0, wgpu::VertexFormat::Float32, 10),
                        attribute(
                            2 * wgpu::VertexFormat::Float32.size(),
                            wgpu::VertexFormat::Float32x2,
                            15,
                        ),
                    ],
                },
                // features
                VertexBufferLayout {
                    array_stride: std::mem::size_of::<FillShaderFeatureMetadata>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: vec![attribute(0, wgpu::VertexFormat::Unorm8x4, 8)],
                },
            ],
        }
    }

    fn describe_fragment(&self) -> FragmentState {
        FragmentState {
            source: include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/render/shaders/fill.fragment.wgsl"
            )),
            entry_point: "main",
            targets: vec![Some(wgpu::ColorTargetState {
                format: self.format,
                // Fill colours arrive premultiplied; blending them over what is already drawn
                // keeps translucent fills composited and the target's alpha at one where the
                // background covers, which the terrain drape relies on to stay opaque.
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }
    }
}
