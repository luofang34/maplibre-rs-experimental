use super::{
    attribute, FillShaderFeatureMetadata, Shader, ShaderLayerMetadata, ShaderTileMetadata,
    ShaderVertex,
};
use crate::render::resource::{FragmentState, VertexBufferLayout, VertexState};

pub struct FillShader {
    pub format: wgpu::TextureFormat,
}

impl Shader for FillShader {
    fn describe_vertex(&self) -> VertexState {
        VertexState {
            source: concat!(
                include_str!("projection.vertex.wgsl"),
                include_str!("fill.vertex.wgsl")
            ),
            entry_point: "main",
            buffers: vec![
                // vertex data
                VertexBufferLayout {
                    array_stride: std::mem::size_of::<ShaderVertex>() as u64,
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
                VertexBufferLayout {
                    array_stride: std::mem::size_of::<ShaderTileMetadata>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: vec![
                        attribute(0, wgpu::VertexFormat::Float32x4, 4),
                        attribute(
                            1 * wgpu::VertexFormat::Float32x4.size(),
                            wgpu::VertexFormat::Float32x4,
                            5,
                        ),
                        attribute(
                            2 * wgpu::VertexFormat::Float32x4.size(),
                            wgpu::VertexFormat::Float32x4,
                            6,
                        ),
                        attribute(
                            3 * wgpu::VertexFormat::Float32x4.size(),
                            wgpu::VertexFormat::Float32x4,
                            7,
                        ),
                        attribute(
                            4 * wgpu::VertexFormat::Float32x4.size(),
                            wgpu::VertexFormat::Float32,
                            9,
                        ),
                        attribute(
                            4 * wgpu::VertexFormat::Float32x4.size()
                                + 3 * wgpu::VertexFormat::Float32.size(),
                            wgpu::VertexFormat::Float32x4,
                            2,
                        ),
                    ],
                },
                // layer metadata
                VertexBufferLayout {
                    array_stride: std::mem::size_of::<ShaderLayerMetadata>() as u64,
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
                    attributes: vec![attribute(0, wgpu::VertexFormat::Float32x4, 8)],
                },
            ],
        }
    }

    fn describe_fragment(&self) -> FragmentState {
        FragmentState {
            source: include_str!("fill.fragment.wgsl"),
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
