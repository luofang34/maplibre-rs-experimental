//! Tile-space vertex geometry shared by globe mesh subdivision and GPU vector pipelines.

use bytemuck_derive::{Pod, Zeroable};

/// Tile-space geometry shared by fills, lines and circles.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct ShaderVertex {
    /// Tile-local position in the renderer's 4096-unit grid.
    pub position: [f32; 2],
    /// Extrusion direction for polygon/line vertices; circle vertices store radius and stroke width in pixels.
    pub normal: [f32; 2],
    /// Distance along a stroked path, in tile units.
    pub distance: f32,
    /// Height above the body datum for spatial lines, and for circles and heatmap points the
    /// ground under them; a float below -1e20 keeps cartographic draping and then carries a
    /// stroke's side, gap width and blur.
    pub elevation: f32,
    /// Distance along the ring of an extruded wall, in tile units; a stroke packs its
    /// per-feature width and offset here instead. Zero elsewhere.
    pub edge_distance: f32,
    /// For a vertex of a subdivided circle or heatmap kernel, one plus the packed position of
    /// its corner on an eight-by-eight grid; zero where the shader decodes the corner from the
    /// vertex index. A circle's fractional part carries a per-feature stroke opacity.
    pub corner_code: f32,
}

impl ShaderVertex {
    /// Creates a vertex with zero path distance and the sentinel for cartographic draping.
    pub fn new(position: [f32; 2], normal: [f32; 2]) -> Self {
        Self {
            position,
            normal,
            distance: 0.0,
            elevation: -1e30,
            edge_distance: 0.0,
            corner_code: 0.0,
        }
    }
}

impl Default for ShaderVertex {
    fn default() -> Self {
        ShaderVertex::new([0.0, 0.0], [0.0, 0.0])
    }
}
