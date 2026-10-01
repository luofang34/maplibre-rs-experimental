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
    /// `circle-blur`, when it varies by feature; otherwise the layer's value serves.
    pub blur: Option<StyleProperty<f32>>,
    /// `circle-stroke-opacity`, when it varies by feature.
    pub stroke_opacity: Option<StyleProperty<f32>>,
    /// Zoom of the tile, at which zoom-driven properties are evaluated.
    pub zoom: f64,
}

/// The property when it needs each feature's attributes.
fn per_feature(property: Option<&StyleProperty<f32>>) -> Option<StyleProperty<f32>> {
    property
        .filter(|property| !property.is_feature_constant())
        .cloned()
}

impl CircleOptions {
    /// Options for a circle paint at a tile zoom.
    pub fn for_paint(paint: &CirclePaint, zoom: f64) -> Self {
        Self {
            radius: paint.radius(),
            radius_default: CirclePaint::DEFAULT_RADIUS,
            stroke_width: paint.stroke_width(),
            stroke_width_default: 0.0,
            blur: per_feature(paint.circle_blur.as_ref()),
            stroke_opacity: per_feature(paint.circle_stroke_opacity.as_ref()),
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
            blur: None,
            stroke_opacity: None,
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
        // A radius that varies by feature and by zoom is stored for the tile's zoom and the one
        // above it, which the shader blends by the fractional zoom, as GL JS does.
        let radius_next = if options.radius.is_feature_constant() {
            radius
        } else {
            options
                .radius
                .evaluate_for(&self.feature_properties, options.zoom + 1.0)
                .unwrap_or(options.radius_default)
                .max(0.0)
        };
        let stroke_width = options
            .stroke_width
            .evaluate_for(&self.feature_properties, options.zoom)
            .unwrap_or(options.stroke_width_default)
            .max(0.0);

        // Negative values tell the shader to keep the layer's own blur and stroke opacity.
        let per_feature = |property: &Option<StyleProperty<f32>>| {
            property
                .as_ref()
                .and_then(|property| property.evaluate_for(&self.feature_properties, options.zoom))
                .map_or(-1.0, |value| value.max(0.0))
        };
        let (blur, stroke_opacity) = (
            per_feature(&options.blur),
            per_feature(&options.stroke_opacity),
        );
        let base = self.buffer.vertices.len() as u32;
        for _ in 0..4 {
            let mut vertex = ShaderVertex::new([x, y], [radius, stroke_width]);
            vertex.distance = blur;
            vertex.elevation = stroke_opacity;
            vertex.edge_distance = radius_next;
            self.buffer.vertices.push(vertex);
        }
        for offset in CIRCLE_QUAD_INDICES {
            self.buffer.indices.push(I::from(VertexId(base + offset)));
        }
    }
}
