use super::Shader;
use crate::render::resource::{FragmentState, VertexBufferLayout, VertexState};

/// Terrain mesh shader: elevation from a DEM texture, color from a drape texture.
pub struct TerrainShader {
    pub format: wgpu::TextureFormat,
}

impl Shader for TerrainShader {
    fn describe_vertex(&self) -> VertexState {
        VertexState {
            source: concat!(
                include_str!("projection.vertex.wgsl"),
                include_str!("terrain.vertex.wgsl")
            ),
            entry_point: "main",
            buffers: vec![VertexBufferLayout {
                array_stride: std::mem::size_of::<crate::terrain::mesh::TerrainVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: vec![
                    wgpu::VertexAttribute {
                        offset: 0,
                        format: wgpu::VertexFormat::Sint16x2,
                        shader_location: 0,
                    },
                    wgpu::VertexAttribute {
                        offset: wgpu::VertexFormat::Sint16x2.size(),
                        format: wgpu::VertexFormat::Uint16x2,
                        shader_location: 1,
                    },
                ],
            }],
        }
    }

    fn describe_fragment(&self) -> FragmentState {
        FragmentState {
            source: include_str!("terrain.fragment.wgsl"),
            entry_point: "main",
            targets: vec![Some(wgpu::ColorTargetState {
                format: self.format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }
    }
}
