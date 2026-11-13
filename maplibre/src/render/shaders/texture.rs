use super::{Shader, ShaderTileMetadata};
use crate::render::resource::{FragmentState, VertexBufferLayout, VertexState};

/// Raster and DEM draws bind the subdivided tile mesh at slot zero and tile metadata at slot one.
pub fn tile_texture_vertex_buffers() -> Vec<VertexBufferLayout> {
    let mut tile_attrs = ShaderTileMetadata::transform_attributes().to_vec();
    tile_attrs.extend([
        // How far a tile drawn into a drape texture is enlarged; one elsewhere.
        super::attribute(
            std::mem::offset_of!(ShaderTileMetadata, line_width_scale) as u64,
            wgpu::VertexFormat::Float32,
            9,
        ),
        super::attribute(
            std::mem::offset_of!(ShaderTileMetadata, tile_mercator_coords) as u64,
            wgpu::VertexFormat::Float32x4,
            2,
        ),
    ]);
    vec![
        crate::render::tile_mesh::GpuTileMeshVertex::vertex_buffer_layout(),
        ShaderTileMetadata::instance_layout(tile_attrs),
    ]
}

/// Raster tile shader using subdivided tile meshes and image sampling.
pub struct RasterShader {
    /// Format of the color attachment used by the render pipeline.
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
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
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
