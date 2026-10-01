//! Shader descriptors of the heatmap density pass and its colourising composite.

use super::{Shader, ShaderLayerMetadata, ShaderTileMetadata};
use crate::render::{
    resource::{FragmentState, VertexBufferLayout, VertexState},
    ShaderVertex,
};

/// Format of the density target. Half floats render and blend without device features.
pub const DENSITY_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;

/// Draws each point as a Gaussian kernel, added into the density target.
pub struct HeatmapDensityShader;

impl Shader for HeatmapDensityShader {
    fn describe_vertex(&self) -> VertexState {
        VertexState {
            source: concat!(
                include_str!("projection.vertex.wgsl"),
                include_str!("heatmap_density.vertex.wgsl")
            ),
            entry_point: "main",
            buffers: vec![
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
                            offset: std::mem::offset_of!(ShaderVertex, normal) as u64,
                            format: wgpu::VertexFormat::Float32x4,
                            shader_location: 1,
                        },
                        wgpu::VertexAttribute {
                            offset: std::mem::offset_of!(ShaderVertex, corner_code) as u64,
                            format: wgpu::VertexFormat::Float32,
                            shader_location: 10,
                        },
                    ],
                },
                VertexBufferLayout {
                    array_stride: std::mem::size_of::<ShaderTileMetadata>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    // The viewport size sits between the zoom factor and the mercator extent, and
                    // only listing it puts the extent at its offset.
                    attributes: wgpu::vertex_attr_array![
                        4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4,
                        9 => Float32, 11 => Float32, 12 => Float32, 2 => Float32x4
                    ]
                    .to_vec(),
                },
                VertexBufferLayout {
                    array_stride: std::mem::size_of::<ShaderLayerMetadata>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: vec![
                        wgpu::VertexAttribute {
                            offset: std::mem::offset_of!(ShaderLayerMetadata, line_width) as u64,
                            format: wgpu::VertexFormat::Float32,
                            shader_location: 13,
                        },
                        wgpu::VertexAttribute {
                            offset: std::mem::offset_of!(ShaderLayerMetadata, circle_params) as u64,
                            format: wgpu::VertexFormat::Float32x4,
                            shader_location: 14,
                        },
                    ],
                },
            ],
        }
    }

    fn describe_fragment(&self) -> FragmentState {
        FragmentState {
            source: include_str!("heatmap_density.fragment.wgsl"),
            entry_point: "main",
            targets: vec![Some(wgpu::ColorTargetState {
                format: DENSITY_FORMAT,
                // Overlapping kernels add up; that sum is the density.
                blend: Some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::One,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::One,
                        operation: wgpu::BlendOperation::Add,
                    },
                }),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }
    }
}

/// Looks each density up in the colour ramp and draws the result over the layers below.
pub struct HeatmapCompositeShader {
    /// Colour format of the render target the heatmap composites into.
    pub format: wgpu::TextureFormat,
}

impl Shader for HeatmapCompositeShader {
    fn describe_vertex(&self) -> VertexState {
        VertexState {
            source: include_str!("heatmap_composite.vertex.wgsl"),
            entry_point: "main",
            buffers: vec![],
        }
    }

    fn describe_fragment(&self) -> FragmentState {
        FragmentState {
            source: include_str!("heatmap_composite.fragment.wgsl"),
            entry_point: "main",
            targets: vec![Some(wgpu::ColorTargetState {
                format: self.format,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }
    }
}
