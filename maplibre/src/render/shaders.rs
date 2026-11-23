//! Vertex formats and shader entry points for map render passes.

#![deny(missing_docs)]
mod background;
mod circle;
mod fill;
mod fill_extrusion;
mod gpu_vertex;
mod heatmap;
mod line;
mod symbol;
mod symbol_vertex;
mod terrain;
mod texture;
mod tile_mask;
mod vertex;

pub use background::{
    AtmosphereLayerMetadata, AtmosphereShader, BackgroundLayerMetadata, BackgroundPatternShader,
    BackgroundShader, GlobeBackgroundPatternShader, GlobeBackgroundShader, SkyLayerMetadata,
    SkyShader,
};
pub use circle::{
    pack_dynamic_circle_radii, pack_dynamic_circle_radius_uniform, CircleDynamicRadiusShader,
    CircleFeatureStrokeShader, CircleShader,
};
pub use fill::{FillPatternShader, FillShader};
pub use fill_extrusion::{
    ExtrusionPass, FillExtrusionCompactShader, FillExtrusionShader, StencilClearShader,
};
pub use gpu_vertex::{
    can_encode_compact_extrusion, encode_compact_extrusion_vertices, f16_bits_to_f32,
    f32_to_f16_bits, pack_f16_pair, unpack_compact_extrusion_vertices, EmptyLayerMetadata,
    GpuShaderVertex, PoolVertex,
};
pub(crate) use gpu_vertex::{
    encode_compact_extrusion_words, is_compact_extrusion_vertex, unpack_compact_extrusion_word,
    GpuVertexKind,
};
pub use heatmap::{HeatmapCompositeShader, HeatmapDensityShader, DENSITY_FORMAT};
pub use line::{LineShader, PlainLineShader, TerrainLineShader};
pub use symbol::SymbolShader;
pub use symbol_vertex::ShaderSymbolVertex;
pub use terrain::TerrainShader;
pub use texture::{tile_texture_vertex_buffers, DemShader, DemShading, RasterShader};
pub use tile_mask::{FlatTileMaskShader, TileMaskShader};
pub use vertex::{
    FillShaderFeatureMetadata, SDFShaderFeatureMetadata, ShaderLayerMetadata, ShaderTileMetadata,
    ShaderVertex, SymbolPlacementMetadata, POSED_GLYPH, WHOLE_TILE,
};

use crate::render::resource::{FragmentState, VertexBufferLayout, VertexState};

/// Two-component GPU vector in tightly packed scalar order.
pub type Vec2f32 = [f32; 2];
/// Three-component GPU vector; world-coordinate conversion sets its third component to zero.
pub type Vec3f32 = [f32; 3];
/// Four-component GPU vector in tightly packed scalar order.
pub type Vec4f32 = [f32; 4];
/// Column-major GPU matrix, with one four-component vector per column.
pub type Mat4x4f32 = [Vec4f32; 4];

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

#[cfg(test)]
mod tests;
