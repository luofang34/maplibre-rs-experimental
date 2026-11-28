//! Vertex formats and shader entry points for map render passes.

#![deny(missing_docs)]
mod background;
mod circle;
mod fill;
mod fill_extrusion;
mod heatmap;
mod line;
mod symbol;
mod terrain;
mod texture;
mod tile_mask;

pub use background::{
    AtmosphereLayerMetadata, AtmosphereShader, BackgroundLayerMetadata, BackgroundPatternShader,
    BackgroundShader, GlobeBackgroundPatternShader, GlobeBackgroundShader, GlobeBackgroundUniform,
    SkyLayerMetadata, SkyShader,
};
pub use circle::{
    pack_dynamic_circle_radii, pack_dynamic_circle_radius_uniform, CircleDynamicRadiusShader,
    CircleFeatureStrokeShader, CircleShader,
};
pub use fill::{FillPatternShader, FillShader};
pub use fill_extrusion::{
    ExtrusionPass, FillExtrusionCompactShader, FillExtrusionShader, StencilClearShader,
};
pub use heatmap::{HeatmapCompositeShader, HeatmapDensityShader, DENSITY_FORMAT};
pub use line::{LineShader, PlainLineShader, TerrainLineShader};
pub use symbol::SymbolShader;
pub use terrain::{TerrainShader, TerrainVertex};
pub use texture::{tile_texture_vertex_buffers, DemShader, DemShading, RasterShader};
pub use tile_mask::{FlatTileMaskShader, TileMaskShader};

pub(crate) use crate::render::resource::vertex_buffer::{
    formats::attribute,
    gpu_vertex::{
        encode_compact_extrusion_words, is_compact_extrusion_vertex, unpack_compact_extrusion_word,
        GpuVertexKind,
    },
};
pub use crate::render::resource::vertex_buffer::{
    formats::{
        f32_to_f16_bits, FillShaderFeatureMetadata, Mat4x4f32, SDFShaderFeatureMetadata,
        ShaderLayerMetadata, ShaderTileMetadata, ShaderVertex, SymbolPlacementMetadata, Vec2f32,
        Vec4f32, POSED_GLYPH, WHOLE_TILE,
    },
    gpu_vertex::{
        can_encode_compact_extrusion, unpack_compact_extrusion_vertices, EmptyLayerMetadata,
        GpuShaderVertex, PoolVertex,
    },
    symbol_vertex::ShaderSymbolVertex,
};
use crate::render::resource::{FragmentState, VertexBufferLayout, VertexState};

/// Three-component GPU vector; world-coordinate conversion sets its third component to zero.
pub type Vec3f32 = [f32; 3];

/// Describes WGSL stages and their vertex, attachment and blend interfaces for pipeline creation.
/// Descriptors must match the data uploaded by the corresponding rendering path.
pub trait Shader {
    /// Vertex entry point and buffer strides, step modes, formats and shader locations.
    fn describe_vertex(&self) -> VertexState;
    /// Fragment entry point with render-target formats, write masks and blending.
    fn describe_fragment(&self) -> FragmentState;
}

#[cfg(test)]
mod tests;
