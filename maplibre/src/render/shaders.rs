//! Vertex formats and shader entry points for map render passes.
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

use crate::{
    coords::WorldCoords,
    render::resource::{FragmentState, VertexBufferLayout, VertexState},
};
pub use background::{
    AtmosphereLayerMetadata, AtmosphereShader, BackgroundLayerMetadata, BackgroundShader,
    GlobeBackgroundShader, SkyLayerMetadata, SkyShader,
};
pub use circle::CircleShader;
pub use fill::FillShader;
pub use line::LineShader;
pub use symbol::SymbolShader;
pub use symbol_vertex::{ShaderSymbolVertex, ShaderSymbolVertexNew};
pub use terrain::TerrainShader;
pub use texture::{tile_texture_vertex_buffers, DemShader, DemShading, RasterShader};
pub use tile_mask::TileMaskShader;
pub use vertex::{
    FillShaderFeatureMetadata, SDFShaderFeatureMetadata, ShaderCamera, ShaderGlobals,
    ShaderLayerMetadata, ShaderTextureVertex, ShaderTileMetadata, ShaderVertex,
};

pub type Vec2f32 = [f32; 2];
pub type Vec3f32 = [f32; 3];
pub type Vec4f32 = [f32; 4];
pub type Mat4x4f32 = [Vec4f32; 4];

impl From<WorldCoords> for Vec3f32 {
    fn from(world_coords: WorldCoords) -> Self {
        [world_coords.x as f32, world_coords.y as f32, 0.0]
    }
}

pub trait Shader {
    fn describe_vertex(&self) -> VertexState;
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
