use super::{Shader, ShaderTileMetadata};
use crate::render::resource::{FragmentState, VertexState};

/// Tile stencil shader with optional diagnostic colors and outlines.
pub struct TileMaskShader {
    /// Format of the color attachment used by the render pipeline.
    pub format: wgpu::TextureFormat,
    /// Enables writes to the color attachment in addition to stencil updates.
    pub draw_colors: bool,
    /// Uses generated outline vertices instead of the indexed tile mesh.
    pub debug_lines: bool,
}

/// Mercator/drape stencil shader omitting fragment `discard` so mobile TBDR GPUs execute stencil
/// updates purely in hardware early-stencil without late-Z coverage drops.
pub struct FlatTileMaskShader {
    /// Format of the color attachment used by the render pipeline.
    pub format: wgpu::TextureFormat,
}

impl Shader for FlatTileMaskShader {
    fn describe_vertex(&self) -> VertexState {
        TileMaskShader {
            format: self.format,
            draw_colors: false,
            debug_lines: false,
        }
        .describe_vertex()
    }

    fn describe_fragment(&self) -> FragmentState {
        FragmentState {
            source: include_str!("tile_mask_flat.fragment.wgsl"),
            entry_point: "main",
            targets: vec![Some(wgpu::ColorTargetState {
                format: self.format,
                blend: None,
                write_mask: wgpu::ColorWrites::empty(),
            })],
        }
    }
}

impl Shader for TileMaskShader {
    fn describe_vertex(&self) -> VertexState {
        let metadata = ShaderTileMetadata::basic_layout();
        let buffers = if self.debug_lines {
            vec![metadata]
        } else {
            vec![
                crate::render::tile_mesh::GpuTileMeshVertex::vertex_buffer_layout(),
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
