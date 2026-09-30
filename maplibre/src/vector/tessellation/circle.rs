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
    style::{circle::CirclePaint, heatmap::HeatmapPaint, layer::StyleProperty},
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
    /// What the radius is when the property has no value for a feature.
    pub radius_default: f32,
    /// What the stroke-width slot holds when the property has no value for a feature.
    pub stroke_width_default: f32,
    /// Zoom of the tile, at which zoom-driven properties are evaluated.
    pub zoom: f64,
}

impl CircleOptions {
    /// Options for a circle paint at a tile zoom.
    pub fn for_paint(paint: &CirclePaint, zoom: f64) -> Self {
        Self {
            radius: paint.radius(),
            radius_default: CirclePaint::DEFAULT_RADIUS,
            stroke_width: paint.stroke_width(),
            stroke_width_default: 0.0,
            zoom,
        }
    }

    /// Options for a heatmap point: a quad whose weight travels in the slot circles use for
    /// the stroke width. A radius that does not depend on the feature is one value for the
    /// whole layer, taken from the layer's instance record, so the quad is one unit; a
    /// per-feature radius is the quad's own size, and the layer's factor is then one. A
    /// feature without a weight counts once.
    pub fn for_heatmap(paint: &HeatmapPaint, zoom: f64) -> Self {
        let per_feature = paint.radius_per_feature();
        Self {
            radius: match &paint.heatmap_radius {
                Some(radius) if per_feature => radius.clone(),
                _ => StyleProperty::Constant(1.0),
            },
            radius_default: if per_feature { 30.0 } else { 1.0 },
            stroke_width: paint
                .heatmap_weight
                .clone()
                .unwrap_or(StyleProperty::Constant(1.0)),
            stroke_width_default: 1.0,
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
            .evaluate_for(&self.feature_properties, options.zoom)
            .unwrap_or(options.radius_default)
            .max(0.0);
        let stroke_width = options
            .stroke_width
            .evaluate_for(&self.feature_properties, options.zoom)
            .unwrap_or(options.stroke_width_default)
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
