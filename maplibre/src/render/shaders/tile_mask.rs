use super::{attribute, Shader, ShaderTileMetadata};
use crate::render::resource::{FragmentState, VertexBufferLayout, VertexState};

pub struct TileMaskShader {
    pub format: wgpu::TextureFormat,
    pub draw_colors: bool,
    pub debug_lines: bool,
}

impl Shader for TileMaskShader {
    fn describe_vertex(&self) -> VertexState {
        let metadata = VertexBufferLayout {
            array_stride: std::mem::size_of::<ShaderTileMetadata>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: vec![
                attribute(0, wgpu::VertexFormat::Float32x4, 4),
                attribute(
                    wgpu::VertexFormat::Float32x4.size(),
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
        };
        let buffers = if self.debug_lines {
            vec![metadata]
        } else {
            vec![
                VertexBufferLayout {
                    array_stride: std::mem::size_of::<
                        crate::projection::globe::tile_mesh::TileMeshVertex,
                    >() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: vec![attribute(0, wgpu::VertexFormat::Sint16x2, 0)],
                },
                metadata,
            ]
        };
        VertexState {
            source: if self.debug_lines {
                concat!(
                    include_str!("projection.vertex.wgsl"),
                    include_str!("tile_debug.vertex.wgsl")
                )
            } else {
                concat!(
                    include_str!("projection.vertex.wgsl"),
                    include_str!("tile_mask.vertex.wgsl")
                )
            },
            entry_point: "main",
            buffers,
        }
    }

    fn describe_fragment(&self) -> FragmentState {
        FragmentState {
            source: include_str!("tile_mask.fragment.wgsl"),
            entry_point: "main",
            targets: vec![Some(wgpu::ColorTargetState {
                format: self.format,
                blend: None,
                write_mask: if self.draw_colors {
                    wgpu::ColorWrites::ALL
                } else {
                    wgpu::ColorWrites::empty()
                },
            })],
        }
    }
}
