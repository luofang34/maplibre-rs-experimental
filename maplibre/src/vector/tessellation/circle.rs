//! Circle layers: one screen-aligned quad per point, extruded by the shader.
//!
//! Each quad's vertices carry the feature's radius and stroke width in the normal slot; the
//! shader recovers the corner direction from the vertex index, so both values can be data
//! driven. Points outside the tile are skipped as GL JS does, since a neighbouring tile draws
//! them.

use lyon::tessellation::{geometry_builder::MaxIndex, VertexId};

use super::ZeroTessellator;
use crate::{
    coords::EXTENT,
    render::ShaderVertex,
    style::{circle::CirclePaint, layer::StyleProperty},
};

/// Vertex order of a circle quad; corners follow the shader's `vertex_index % 4` decoding.
pub const CIRCLE_QUAD_INDICES: [u32; 6] = [0, 1, 2, 0, 2, 3];

/// What a circle layer needs to size each feature's quad.
#[derive(Clone, Debug)]
pub struct CircleOptions {
    /// Radius in screen pixels, evaluated per feature.
    pub radius: StyleProperty<f32>,
    /// Stroke width in screen pixels, evaluated per feature.
    pub stroke_width: StyleProperty<f32>,
    /// Zoom of the tile, at which zoom-driven properties are evaluated.
    pub zoom: f64,
}

impl CircleOptions {
    /// Options for a circle paint at a tile zoom.
    pub fn for_paint(paint: &CirclePaint, zoom: f64) -> Self {
        Self {
            radius: paint.radius(),
            stroke_width: paint.stroke_width(),
            zoom,
        }
    }
}

impl<I> ZeroTessellator<I>
where
    I: std::ops::Add + From<VertexId> + MaxIndex + Copy + Into<u32>,
{
    /// Turns every point of every feature into a circle quad instead of a path.
    pub fn with_circles(mut self, options: CircleOptions) -> Self {
        self.circle = Some(options);
        self
    }

    pub(super) fn emit_circle(&mut self, x: f32, y: f32) {
        let Some(options) = &self.circle else {
            return;
        };
        let extent = EXTENT as f32;
        if !(0.0..extent).contains(&x) || !(0.0..extent).contains(&y) {
            return;
        }
        let radius = options
            .radius
            .evaluate_number(&self.feature_properties, options.zoom)
            .unwrap_or(CirclePaint::DEFAULT_RADIUS)
            .max(0.0);
        let stroke_width = options
            .stroke_width
            .evaluate_number(&self.feature_properties, options.zoom)
            .unwrap_or(0.0)
            .max(0.0);

        let base = self.buffer.vertices.len() as u32;
        for _ in 0..4 {
            self.buffer
                .vertices
                .push(ShaderVertex::new([x, y], [radius, stroke_width]));
        }
        for offset in CIRCLE_QUAD_INDICES {
            self.buffer.indices.push(I::from(VertexId(base + offset)));
        }
    }
}
