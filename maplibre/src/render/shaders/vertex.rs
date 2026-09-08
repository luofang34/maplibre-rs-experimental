use super::{Mat4x4f32, Vec2f32, Vec4f32};
use bytemuck_derive::{Pod, Zeroable};
use cgmath::SquareMatrix;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct ShaderCamera {
    view_proj: Mat4x4f32,   // 64 bytes
    view_position: Vec4f32, // 16 bytes
}

impl ShaderCamera {
    pub fn new(view_proj: Mat4x4f32, view_position: Vec4f32) -> Self {
        Self {
            view_position,
            view_proj,
        }
    }
}

impl Default for ShaderCamera {
    fn default() -> Self {
        Self {
            view_position: [0.0; 4],
            view_proj: cgmath::Matrix4::identity().into(),
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct ShaderGlobals {
    camera: ShaderCamera,
}

impl ShaderGlobals {
    pub fn new(camera_uniform: ShaderCamera) -> Self {
        Self {
            camera: camera_uniform,
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct ShaderVertex {
    pub position: Vec2f32,
    pub normal: Vec2f32,
    /// Distance along a stroked path, in tile units.
    pub distance: f32,
}

impl ShaderVertex {
    pub fn new(position: Vec2f32, normal: Vec2f32) -> Self {
        Self {
            position,
            normal,
            distance: 0.0,
        }
    }
}

impl Default for ShaderVertex {
    fn default() -> Self {
        ShaderVertex::new([0.0, 0.0], [0.0, 0.0])
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
pub struct FillShaderFeatureMetadata {
    pub color: Vec4f32,
}

#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable, Default)]
pub struct SDFShaderFeatureMetadata {
    pub opacity: f32,
    pub elevation: f32,
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct ShaderLayerMetadata {
    pub z_index: f32,
    pub line_width: f32,
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

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct ShaderTileMetadata {
    pub transform: Mat4x4f32,
    pub zoom_factor: f32,
    pub viewport_width: f32,
    pub viewport_height: f32,
    pub tile_mercator_coords: Vec4f32,
    pub clip_antimeridian: u32,
    /// Converts style pixels to pixels in an offscreen drape texture.
    pub line_width_scale: f32,
    /// Tile units per texture or viewport pixel for line dash lengths.
    pub line_units_per_pixel: f32,
}

impl ShaderTileMetadata {
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

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct ShaderTextureVertex {
    pub position: Vec2f32,
    pub tex_coords: Vec2f32,
}

impl ShaderTextureVertex {
    pub fn new(position: Vec2f32, tex_coords: Vec2f32) -> Self {
        Self {
            position,
            tex_coords,
        }
    }
}

impl Default for ShaderTextureVertex {
    fn default() -> Self {
        ShaderTextureVertex::new([0.0, 0.0], [0.0, 0.0])
    }
}
