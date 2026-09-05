//! Circle shader descriptor: the vector buffer layout with the circle slots of the layer
//! metadata exposed.

use super::{FillShaderFeatureMetadata, Shader, ShaderLayerMetadata, ShaderTileMetadata};
use crate::render::{
    resource::{FragmentState, VertexBufferLayout, VertexState},
    ShaderVertex,
};

/// The circle layer shader pair; the fragment output matches `format`.
pub struct CircleShader {
    /// Colour format of the render target the circles draw into.
    pub format: wgpu::TextureFormat,
}

impl Shader for CircleShader {
    fn describe_vertex(&self) -> VertexState {
        let float4 = wgpu::VertexFormat::Float32x4.size();
        let float = wgpu::VertexFormat::Float32.size();
        VertexState {
            source: concat!(
                include_str!("projection.vertex.wgsl"),
                include_str!("circle.vertex.wgsl")
            ),
            entry_point: "main",
            buffers: vec![
                // vertex data
                VertexBufferLayout {
                    array_stride: std::mem::size_of::<ShaderVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: vec![
                        wgpu::VertexAttribute {
                            offset: 0,
                            format: wgpu::VertexFormat::Float32x2,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            offset: wgpu::VertexFormat::Float32x2.size(),
                            format: wgpu::VertexFormat::Float32x2,
                            shader_location: 1,
                        },
                    ],
                },
                // tile metadata
                VertexBufferLayout {
                    array_stride: std::mem::size_of::<ShaderTileMetadata>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: vec![
                        wgpu::VertexAttribute {
                            offset: 0,
                            format: wgpu::VertexFormat::Float32x4,
                            shader_location: 4,
                        },
                        wgpu::VertexAttribute {
                            offset: float4,
                            format: wgpu::VertexFormat::Float32x4,
                            shader_location: 5,
                        },
                        wgpu::VertexAttribute {
                            offset: 2 * float4,
                            format: wgpu::VertexFormat::Float32x4,
                            shader_location: 6,
                        },
                        wgpu::VertexAttribute {
                            offset: 3 * float4,
                            format: wgpu::VertexFormat::Float32x4,
                            shader_location: 7,
                        },
                        // zoom_factor
                        wgpu::VertexAttribute {
                            offset: 4 * float4,
                            format: wgpu::VertexFormat::Float32,
                            shader_location: 9,
                        },
                        // viewport_width
                        wgpu::VertexAttribute {
                            offset: 4 * float4 + float,
                            format: wgpu::VertexFormat::Float32,
                            shader_location: 11,
                        },
                        // viewport_height
                        wgpu::VertexAttribute {
                            offset: 4 * float4 + 2 * float,
                            format: wgpu::VertexFormat::Float32,
                            shader_location: 12,
                        },
                        // tile_mercator_coords
                        wgpu::VertexAttribute {
                            offset: 4 * float4 + 3 * float,
                            format: wgpu::VertexFormat::Float32x4,
                            shader_location: 2,
                        },
                    ],
                },
                // layer metadata
                VertexBufferLayout {
                    array_stride: std::mem::size_of::<ShaderLayerMetadata>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: vec![
                        // z_index
                        wgpu::VertexAttribute {
                            offset: 0,
                            format: wgpu::VertexFormat::Float32,
                            shader_location: 10,
                        },
                        // translate
                        wgpu::VertexAttribute {
                            offset: 2 * float,
                            format: wgpu::VertexFormat::Float32x2,
                            shader_location: 15,
                        },
                        // stroke_color
                        wgpu::VertexAttribute {
                            offset: float4,
                            format: wgpu::VertexFormat::Float32x4,
                            shader_location: 16,
                        },
                        // circle_params
                        wgpu::VertexAttribute {
                            offset: 2 * float4,
                            format: wgpu::VertexFormat::Float32x4,
                            shader_location: 17,
                        },
                        // circle_flags
                        wgpu::VertexAttribute {
                            offset: 3 * float4,
                            format: wgpu::VertexFormat::Float32x4,
                            shader_location: 18,
                        },
                    ],
                },
                // features
                VertexBufferLayout {
                    array_stride: std::mem::size_of::<FillShaderFeatureMetadata>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: vec![wgpu::VertexAttribute {
                        offset: 0,
                        format: wgpu::VertexFormat::Float32x4,
                        shader_location: 8,
                    }],
                },
            ],
        }
    }

    fn describe_fragment(&self) -> FragmentState {
        FragmentState {
            source: include_str!("circle.fragment.wgsl"),
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
