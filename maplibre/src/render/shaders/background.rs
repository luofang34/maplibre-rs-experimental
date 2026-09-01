//! Background, globe surface, and atmospheric shader descriptors.

use bytemuck_derive::{Pod, Zeroable};

use super::{Shader, ShaderTileMetadata};
use crate::render::resource::{FragmentState, VertexBufferLayout, VertexState};

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct BackgroundLayerMetadata {
    pub color: [f32; 4],
    pub z_index: f32,
}

pub struct BackgroundShader {
    pub format: wgpu::TextureFormat,
}

/// Shader drawing background paint on the projected globe surface.
pub struct GlobeBackgroundShader {
    /// Render-target format.
    pub format: wgpu::TextureFormat,
}

/// Per-draw atmospheric opacity.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct AtmosphereLayerMetadata {
    /// Style opacity multiplied by the active globe transition.
    pub blend: f32,
    padding: [f32; 3],
}

impl AtmosphereLayerMetadata {
    /// Creates uniform-safe atmosphere metadata.
    pub fn new(blend: f32) -> Self {
        Self {
            blend,
            padding: [0.0; 3],
        }
    }
}

/// Shader drawing a translucent atmospheric shell.
pub struct AtmosphereShader {
    /// Render-target format.
    pub format: wgpu::TextureFormat,
}

impl Shader for AtmosphereShader {
    fn describe_vertex(&self) -> VertexState {
        VertexState {
            source: concat!(
                include_str!("projection.vertex.wgsl"),
                include_str!("atmosphere.vertex.wgsl")
            ),
            entry_point: "main",
            buffers: atmosphere_buffers(),
        }
    }

    fn describe_fragment(&self) -> FragmentState {
        FragmentState {
            source: include_str!("atmosphere.fragment.wgsl"),
            entry_point: "main",
            targets: vec![Some(wgpu::ColorTargetState {
                format: self.format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }
    }
}

fn atmosphere_buffers() -> Vec<VertexBufferLayout> {
    vec![
        tile_mesh_layout(),
        VertexBufferLayout {
            array_stride: std::mem::size_of::<ShaderTileMetadata>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: vec![wgpu::VertexAttribute {
                offset: 4 * wgpu::VertexFormat::Float32x4.size()
                    + 3 * wgpu::VertexFormat::Float32.size(),
                format: wgpu::VertexFormat::Float32x4,
                shader_location: 2,
            }],
        },
        VertexBufferLayout {
            array_stride: std::mem::size_of::<AtmosphereLayerMetadata>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: vec![wgpu::VertexAttribute {
                offset: 0,
                format: wgpu::VertexFormat::Float32,
                shader_location: 8,
            }],
        },
    ]
}

impl Shader for GlobeBackgroundShader {
    fn describe_vertex(&self) -> VertexState {
        VertexState {
            source: concat!(
                include_str!("projection.vertex.wgsl"),
                include_str!("globe_background.vertex.wgsl")
            ),
            entry_point: "main",
            buffers: globe_background_buffers(),
        }
    }

    fn describe_fragment(&self) -> FragmentState {
        FragmentState {
            source: include_str!("globe_background.fragment.wgsl"),
            entry_point: "main",
            targets: vec![Some(wgpu::ColorTargetState {
                format: self.format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }
    }
}

fn globe_background_buffers() -> Vec<VertexBufferLayout> {
    vec![
        tile_mesh_layout(),
        VertexBufferLayout {
            array_stride: std::mem::size_of::<ShaderTileMetadata>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: vec![
                matrix_attribute(0, 4),
                matrix_attribute(1, 5),
                matrix_attribute(2, 6),
                matrix_attribute(3, 7),
                wgpu::VertexAttribute {
                    offset: 4 * wgpu::VertexFormat::Float32x4.size()
                        + 3 * wgpu::VertexFormat::Float32.size(),
                    format: wgpu::VertexFormat::Float32x4,
                    shader_location: 2,
                },
            ],
        },
        VertexBufferLayout {
            array_stride: std::mem::size_of::<BackgroundLayerMetadata>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: vec![
                wgpu::VertexAttribute {
                    offset: 0,
                    format: wgpu::VertexFormat::Float32x4,
                    shader_location: 8,
                },
                wgpu::VertexAttribute {
                    offset: wgpu::VertexFormat::Float32x4.size(),
                    format: wgpu::VertexFormat::Float32,
                    shader_location: 10,
                },
            ],
        },
    ]
}

fn tile_mesh_layout() -> VertexBufferLayout {
    VertexBufferLayout {
        array_stride: std::mem::size_of::<crate::projection::globe::tile_mesh::TileMeshVertex>()
            as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: vec![wgpu::VertexAttribute {
            offset: 0,
            format: wgpu::VertexFormat::Sint16x2,
            shader_location: 0,
        }],
    }
}

fn matrix_attribute(column: u64, shader_location: u32) -> wgpu::VertexAttribute {
    wgpu::VertexAttribute {
        offset: column * wgpu::VertexFormat::Float32x4.size(),
        format: wgpu::VertexFormat::Float32x4,
        shader_location,
    }
}

impl Shader for BackgroundShader {
    fn describe_vertex(&self) -> VertexState {
        VertexState {
            source: include_str!("background.vertex.wgsl"),
            entry_point: "main",
            buffers: vec![VertexBufferLayout {
                array_stride: std::mem::size_of::<BackgroundLayerMetadata>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: vec![
                    wgpu::VertexAttribute {
                        offset: 0,
                        format: wgpu::VertexFormat::Float32x4,
                        shader_location: 0,
                    },
                    wgpu::VertexAttribute {
                        offset: 16,
                        format: wgpu::VertexFormat::Float32,
                        shader_location: 1,
                    },
                ],
            }],
        }
    }

    fn describe_fragment(&self) -> FragmentState {
        FragmentState {
            source: include_str!("basic.fragment.wgsl"),
            entry_point: "main",
            targets: vec![Some(wgpu::ColorTargetState {
                format: self.format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }
    }
}
