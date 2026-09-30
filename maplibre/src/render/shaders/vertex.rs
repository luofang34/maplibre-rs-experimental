//! Packed CPU records consumed by the vertex layouts of tile rendering pipelines.

use bytemuck_derive::{Pod, Zeroable};

use super::{Mat4x4f32, Vec2f32, Vec4f32};

/// Tile-space geometry shared by fills, lines and circles.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct ShaderVertex {
    /// Tile-local position in the renderer's 4096-unit grid.
    pub position: Vec2f32,
    /// Extrusion direction for polygon/line vertices; circle vertices store radius and stroke width in pixels.
    pub normal: Vec2f32,
    /// Distance along a stroked path, in tile units.
    pub distance: f32,
    /// Height above the body datum for spatial lines; a float below -1e20 keeps cartographic
    /// draping and then carries a stroke's side, gap width and blur.
    pub elevation: f32,
    /// Distance along the ring of an extruded wall, in tile units; a stroke packs its
    /// per-feature width and offset here instead. Zero elsewhere.
    pub edge_distance: f32,
}

impl ShaderVertex {
    /// Creates a vertex with zero path distance and the sentinel for cartographic draping.
    pub fn new(position: Vec2f32, normal: Vec2f32) -> Self {
        Self {
            position,
            normal,
            distance: 0.0,
            elevation: -1e30,
            edge_distance: 0.0,
        }
    }
}

impl Default for ShaderVertex {
    fn default() -> Self {
        ShaderVertex::new([0.0, 0.0], [0.0, 0.0])
    }
}

/// Per-vertex feature color for vector geometry.
#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
pub struct FillShaderFeatureMetadata {
    /// Encoded-sRGB color with straight alpha; feature opacity is folded into alpha.
    pub color: Vec4f32,
}

/// Per-vertex collision visibility and sampled terrain height for symbols.
#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable, Default)]
pub struct SDFShaderFeatureMetadata {
    /// Collision and fade opacity in 0..=1, repeated for the symbol's vertices.
    pub opacity: f32,
    /// Sampled ground elevation in meters; symbol height offsets are carried in the geometry.
    pub elevation: f32,
    /// Where a glyph placed along a line sits: the offset of its centre from the vertex anchor
    /// in tile units, its direction in radians, and 1 when the glyph has such a pose.
    pub pose: [f32; 4],
    /// Fill colour of the symbol's text or icon, straight alpha, evaluated for its feature.
    pub color: [f32; 4],
    /// Halo colour of the text or icon.
    pub halo: [f32; 4],
    /// Size, halo width, halo blur and opacity of the text or icon.
    pub params: [f32; 4],
}

/// Per-layer instance record shared across tile pipelines.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct ShaderLayerMetadata {
    /// Style painter-order index carried by the shared instance layout.
    pub z_index: f32,
    /// Evaluated line width in style pixels, before per-tile drape scaling.
    pub line_width: f32,
    /// Layer translation converted to tile units in map axes.
    pub translate: Vec2f32,
    /// Circle stroke colour as straight RGBA; other layers leave it black.
    pub stroke_color: Vec4f32,
    /// Circle opacity, stroke opacity and blur ratio; the last slot is padding.
    pub circle_params: Vec4f32,
    /// Circle pitch-scale (x) and pitch-alignment (y), 1.0 for `map`; the rest is padding.
    pub circle_flags: Vec4f32,
}

impl ShaderLayerMetadata {
    /// Metadata for a layer without circle paint.
    pub fn new(z_index: f32, line_width: f32, translate: Vec2f32) -> Self {
        Self {
            z_index,
            line_width,
            translate,
            stroke_color: [0.0, 0.0, 0.0, 1.0],
            circle_params: [1.0, 1.0, 0.0, 0.0],
            circle_flags: [0.0; 4],
        }
    }
}

/// Per-tile instance record with projection, viewport and line scaling inputs.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct ShaderTileMetadata {
    /// Column-major tile-to-clip transform used by the flat projection path.
    pub transform: Mat4x4f32,
    /// Scale from view zoom to tile zoom, normally `2^(tile_zoom - view_zoom)`.
    pub zoom_factor: f32,
    /// Render viewport width in pixels.
    pub viewport_width: f32,
    /// Render viewport height in pixels.
    pub viewport_height: f32,
    /// Mercator world origin in XY and per-tile-unit scales in ZW.
    pub tile_mercator_coords: Vec4f32,
    /// One clips geometry outside the root tile at the antimeridian; zero disables clipping.
    pub clip_antimeridian: u32,
    /// Converts style pixels to pixels in an offscreen drape texture.
    pub line_width_scale: f32,
    /// Tile units per texture or viewport pixel for line dash lengths.
    pub line_units_per_pixel: f32,
}

impl ShaderTileMetadata {
    /// Creates root-tile metadata for a 512-pixel viewport without antimeridian clipping.
    /// Callers drawing another tile or viewport must replace those fields before upload.
    pub fn new(transform: Mat4x4f32, zoom_factor: f32) -> Self {
        Self {
            transform,
            zoom_factor,
            viewport_width: 512.0,
            viewport_height: 512.0,
            tile_mercator_coords: [0.0, 0.0, 1.0 / 4096.0, 1.0 / 4096.0],
            clip_antimeridian: 0,
            line_width_scale: 1.0,
            line_units_per_pixel: 8.0 * zoom_factor,
        }
    }
}
