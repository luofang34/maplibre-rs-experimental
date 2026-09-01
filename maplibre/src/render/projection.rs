//! GPU-facing projection data shared by tile shaders.

use bytemuck_derive::{Pod, Zeroable};

use crate::{
    projection::renderer_data::RendererProjectionData,
    render::shaders::{Mat4x4f32, Vec4f32},
};

/// View-wide globe projection values uploaded as a uniform buffer.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct ShaderProjectionData {
    /// Matrix projecting the unit globe to WebGPU clip space.
    pub main_matrix: Mat4x4f32,
    /// Unit-sphere horizon plane.
    pub clipping_plane: Vec4f32,
    /// Mercator-to-globe interpolation factor.
    pub transition: f32,
    padding: [f32; 3],
}

impl ShaderProjectionData {
    /// Creates the view-wide subset of renderer projection data.
    pub fn from_renderer_data(data: RendererProjectionData) -> Self {
        Self {
            main_matrix: data.main_matrix.into(),
            clipping_plane: data.clipping_plane.into(),
            transition: data.projection_transition,
            padding: [0.0; 3],
        }
    }
}

#[cfg(test)]
mod tests;
