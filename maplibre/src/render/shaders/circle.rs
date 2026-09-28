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
        VertexState {
            source: concat!(
                include_str!("projection.vertex.wgsl"),
                include_str!("circle.vertex.wgsl")
            ),
            entry_point: "main",
            buffers: vec![
                VertexBufferLayout {
                    array_stride: std::mem::size_of::<ShaderVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2].to_vec(),
                },
                VertexBufferLayout {
                    array_stride: std::mem::size_of::<ShaderTileMetadata>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: wgpu::vertex_attr_array![
                        4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4,
                        9 => Float32, 11 => Float32, 12 => Float32, 2 => Float32x4
                    ]
                    .to_vec(),
                },
                circle_layer_layout(),
                VertexBufferLayout {
                    array_stride: std::mem::size_of::<FillShaderFeatureMetadata>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: wgpu::vertex_attr_array![8 => Float32x4].to_vec(),
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

fn circle_layer_layout() -> VertexBufferLayout {
    VertexBufferLayout {
        array_stride: std::mem::size_of::<ShaderLayerMetadata>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: vec![
            wgpu::VertexAttribute {
                offset: std::mem::offset_of!(ShaderLayerMetadata, translate) as u64,
                format: wgpu::VertexFormat::Float32x2,
                shader_location: 15,
            },
            wgpu::VertexAttribute {
                offset: std::mem::offset_of!(ShaderLayerMetadata, stroke_color) as u64,
                format: wgpu::VertexFormat::Float32x4,
                shader_location: 3,
            },
            wgpu::VertexAttribute {
                offset: std::mem::offset_of!(ShaderLayerMetadata, circle_params) as u64,
                format: wgpu::VertexFormat::Float32x4,
                shader_location: 13,
            },
            wgpu::VertexAttribute {
                offset: std::mem::offset_of!(ShaderLayerMetadata, circle_flags) as u64,
                format: wgpu::VertexFormat::Float32x4,
                shader_location: 14,
            },
        ],
    }
}
