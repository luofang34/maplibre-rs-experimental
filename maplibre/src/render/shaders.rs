//! Vertex formats and shader entry points for map render passes.

#![deny(missing_docs)]
mod background;
mod circle;
mod fill;
mod line;
mod symbol;
mod symbol_vertex;
mod terrain;
mod texture;
mod tile_mask;
mod vertex;

pub use background::{
    AtmosphereLayerMetadata, AtmosphereShader, BackgroundLayerMetadata, BackgroundShader,
    GlobeBackgroundShader, SkyLayerMetadata, SkyShader,
};
pub use circle::CircleShader;
pub use fill::FillShader;
pub use line::LineShader;
pub use symbol::SymbolShader;
pub use symbol_vertex::ShaderSymbolVertex;
pub use terrain::TerrainShader;
pub use texture::{tile_texture_vertex_buffers, DemShader, DemShading, RasterShader};
pub use tile_mask::TileMaskShader;
pub use vertex::{
    FillShaderFeatureMetadata, SDFShaderFeatureMetadata, ShaderLayerMetadata, ShaderTileMetadata,
    ShaderVertex,
};

use crate::{
    coords::WorldCoords,
    render::resource::{FragmentState, VertexBufferLayout, VertexState},
};

/// Two-component GPU vector in tightly packed scalar order.
pub type Vec2f32 = [f32; 2];
/// Three-component GPU vector; world-coordinate conversion sets its third component to zero.
pub type Vec3f32 = [f32; 3];
/// Four-component GPU vector in tightly packed scalar order.
pub type Vec4f32 = [f32; 4];
/// Column-major GPU matrix, with one four-component vector per column.
pub type Mat4x4f32 = [Vec4f32; 4];

impl From<WorldCoords> for Vec3f32 {
    fn from(world_coords: WorldCoords) -> Self {
        [world_coords.x as f32, world_coords.y as f32, 0.0]
    }
}

/// Describes WGSL stages and their vertex, attachment and blend interfaces for pipeline creation.
/// Descriptors must match the data uploaded by the corresponding rendering path.
pub trait Shader {
    /// Vertex entry point and buffer strides, step modes, formats and shader locations.
    fn describe_vertex(&self) -> VertexState;
    /// Fragment entry point with render-target formats, write masks and blending.
    fn describe_fragment(&self) -> FragmentState;
}

fn attribute(
    offset: u64,
    format: wgpu::VertexFormat,
    shader_location: u32,
) -> wgpu::VertexAttribute {
    wgpu::VertexAttribute {
        offset,
        format,
        shader_location,
    }
}
