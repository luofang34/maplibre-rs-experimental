use super::{Shader, ShaderLayerMetadata, ShaderTileMetadata};
use crate::render::resource::{FragmentState, VertexBufferLayout, VertexState};

/// The vertex buffers every tile-quad texture draw binds: the subdivided tile mesh, the
/// tile metadata and the layer metadata.
pub fn tile_texture_vertex_buffers() -> Vec<VertexBufferLayout> {
    vec![
        // subdivided tile mesh
        VertexBufferLayout {
            array_stride: std::mem::size_of::<crate::projection::globe::tile_mesh::TileMeshVertex>()
                as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: vec![wgpu::VertexAttribute {
                offset: 0,
                format: wgpu::VertexFormat::Sint16x2,
                shader_location: 0,
            }],
        },
        // tile metadata
        VertexBufferLayout {
            array_stride: std::mem::size_of::<ShaderTileMetadata>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: vec![
                // translate
                wgpu::VertexAttribute {
                    offset: 0,
                    format: wgpu::VertexFormat::Float32x4,
                    shader_location: 4,
                },
                wgpu::VertexAttribute {
                    offset: 1 * wgpu::VertexFormat::Float32x4.size(),
                    format: wgpu::VertexFormat::Float32x4,
                    shader_location: 5,
                },
                wgpu::VertexAttribute {
                    offset: 2 * wgpu::VertexFormat::Float32x4.size(),
                    format: wgpu::VertexFormat::Float32x4,
                    shader_location: 6,
                },
                wgpu::VertexAttribute {
                    offset: 3 * wgpu::VertexFormat::Float32x4.size(),
                    format: wgpu::VertexFormat::Float32x4,
                    shader_location: 7,
                },
                // zoom_factor
                wgpu::VertexAttribute {
                    offset: 4 * wgpu::VertexFormat::Float32x4.size(),
                    format: wgpu::VertexFormat::Float32,
                    shader_location: 9,
                },
                // tile_mercator_coords
                wgpu::VertexAttribute {
                    offset: 4 * wgpu::VertexFormat::Float32x4.size()
                        + 3 * wgpu::VertexFormat::Float32.size(),
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
            ],
        },
    ]
}

pub struct RasterShader {
    pub format: wgpu::TextureFormat,
}

impl Shader for RasterShader {
    fn describe_vertex(&self) -> VertexState {
        VertexState {
            source: concat!(
                include_str!("projection.vertex.wgsl"),
                include_str!("raster.vertex.wgsl")
            ),
            entry_point: "main",
            buffers: tile_texture_vertex_buffers(),
        }
    }

    fn describe_fragment(&self) -> FragmentState {
        FragmentState {
            source: include_str!("raster.fragment.wgsl"),
            entry_point: "main",
            targets: vec![Some(wgpu::ColorTargetState {
                format: self.format,
                blend: Some(wgpu::BlendState {
                    color: wgpu::BlendComponent::REPLACE,
                    alpha: wgpu::BlendComponent::REPLACE,
                }),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }
    }
}

/// Which DEM-shaded layer a [`DemShader`] draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DemShading {
    /// Slopes shaded with the layer's lights.
    Hillshade,
    /// Elevation mapped through a colour ramp.
    ColorRelief,
}

/// Shader of the DEM-shaded layers, drawn on the raster tile quad with premultiplied blending.
pub struct DemShader {
    /// Render target format.
    pub format: wgpu::TextureFormat,
    /// Which shading the fragment stage applies.
    pub shading: DemShading,
}

impl Shader for DemShader {
    fn describe_vertex(&self) -> VertexState {
        VertexState {
            source: concat!(
                include_str!("projection.vertex.wgsl"),
                include_str!("dem.vertex.wgsl")
            ),
            entry_point: "main",
            buffers: tile_texture_vertex_buffers(),
        }
    }

    fn describe_fragment(&self) -> FragmentState {
        FragmentState {
            source: match self.shading {
                DemShading::Hillshade => include_str!("hillshade.fragment.wgsl"),
                DemShading::ColorRelief => include_str!("color_relief.fragment.wgsl"),
            },
            entry_point: "main",
            targets: vec![Some(wgpu::ColorTargetState {
                format: self.format,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }
    }
}
