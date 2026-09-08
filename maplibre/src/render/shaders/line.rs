use super::{
    attribute, FillShaderFeatureMetadata, Shader, ShaderLayerMetadata, ShaderTileMetadata,
    ShaderVertex,
};
use crate::render::resource::{FragmentState, VertexBufferLayout, VertexState};

pub struct LineShader {
    pub format: wgpu::TextureFormat,
}

impl Shader for LineShader {
    fn describe_vertex(&self) -> VertexState {
        VertexState {
            source: concat!(
                include_str!("projection.vertex.wgsl"),
                include_str!("line.vertex.wgsl")
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
                            wgpu::VertexFormat::Float32x3,
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
                            std::mem::offset_of!(ShaderTileMetadata, line_width_scale) as u64,
                            wgpu::VertexFormat::Float32x2,
                            9,
                        ),
                        attribute(
                            4 * wgpu::VertexFormat::Float32x4.size()
                                + wgpu::VertexFormat::Float32.size(),
                            wgpu::VertexFormat::Float32,
                            11,
                        ),
                        attribute(
                            4 * wgpu::VertexFormat::Float32x4.size()
                                + 2 * wgpu::VertexFormat::Float32.size(),
                            wgpu::VertexFormat::Float32,
                            12,
                        ),
                        attribute(
                            4 * wgpu::VertexFormat::Float32x4.size()
                                + 3 * wgpu::VertexFormat::Float32.size(),
                            wgpu::VertexFormat::Float32x4,
                            2,
                        ),
                        attribute(
                            5 * wgpu::VertexFormat::Float32x4.size()
                                + 3 * wgpu::VertexFormat::Float32.size(),
                            wgpu::VertexFormat::Uint32,
                            14,
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
                            wgpu::VertexFormat::Float32.size(),
                            wgpu::VertexFormat::Float32,
                            13,
                        ),
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
            source: include_str!("line.fragment.wgsl"),
            entry_point: "main",
            targets: vec![Some(wgpu::ColorTargetState {
                format: self.format,
                blend: Some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::SrcAlpha,
                        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                        operation: wgpu::BlendOperation::Add,
                    },
                }),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }
    }
}
