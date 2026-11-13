//! Circle layers: one screen-aligned quad per point, extruded by the shader.
//!
//! Each quad's vertices carry the feature's radius and stroke width in the normal slot; the
//! shader recovers the corner direction from the vertex index, so both values can be data
//! driven. Points outside the tile are skipped as GL JS does, since a neighbouring tile draws
//! them.

use csscolorparser::Color;
use lyon::tessellation::{geometry_builder::MaxIndex, VertexId};

use super::ZeroTessellator;
use crate::{
    coords::EXTENT,
    render::ShaderVertex,
    style::{circle::CirclePaint, heatmap::HeatmapPaint, layer::StyleProperty},
};

/// Positions on the eight-by-eight corner grid of a subdivided circle, which is four by four
/// vertices; GL JS uses these for circles and heatmap kernels that follow the globe.
pub const CIRCLE_GRID_STEPS: [u32; 4] = [0, 2, 5, 7];

/// Vertex order of a circle quad; corners follow the shader's `vertex_index % 4` decoding.
pub const CIRCLE_QUAD_INDICES: [u32; 6] = [0, 1, 2, 0, 2, 3];

/// The fraction of a circle vertex's `corner_code` that carries a per-feature stroke opacity:
/// zero keeps the layer's own, and an opacity `o` is `0.25 + 0.5 o`. The integer part stays the
/// corner, and the elevation slot stays free for the ground the circle stands on.
fn stroke_opacity_fraction(stroke_opacity: f32) -> f32 {
    if stroke_opacity < 0.0 {
        0.0
    } else {
        0.25 + 0.5 * stroke_opacity.min(1.0)
    }
}

/// Two colours, straight RGBA in 0..=1, in the four lanes of one feature colour: each lane holds
/// two 8-bit channels as `-(256 a + b) - 1`, which an `f32` keeps exactly, as GL JS packs
/// colours. The vertex slots of a circle are all taken, so a stroke colour that varies by
/// feature shares the lanes of the fill's. A plain colour is never negative, so the sign tells
/// the shader which a vertex holds, whatever the layer's style says now: a tile built before a
/// paint change keeps drawing as it was built until its replacement arrives.
pub fn pack_circle_colors(fill: [f32; 4], stroke: [f32; 4]) -> [f32; 4] {
    let byte = |channel: f32| (channel.clamp(0.0, 1.0) * 255.0).round();
    let pair = |high: f32, low: f32| -(byte(high) * 256.0 + byte(low)) - 1.0;
    [
        pair(fill[0], fill[1]),
        pair(fill[2], fill[3]),
        pair(stroke[0], stroke[1]),
        pair(stroke[2], stroke[3]),
    ]
}

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
    /// `circle-stroke-color`, when it varies by feature; each feature's colour then carries
    /// both its fill and its stroke, as `pack_circle_colors` lays them out.
    pub stroke_color: Option<StyleProperty<Color>>,
    /// Zoom of the tile, at which zoom-driven properties are evaluated.
    pub zoom: f64,
    /// Whether each quad is a grid of vertices, so a circle lying on the globe follows its curve.
    pub grid: bool,
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
            stroke_color: paint.stroke_color_per_feature().cloned(),
            zoom,
            grid: false,
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
            stroke_color: None,
            zoom,
            grid: false,
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

    /// The colour a finished feature's vertices carry: its fill, or its fill and stroke packed
    /// together when the stroke colour varies by feature.
    pub(super) fn circle_feature_color(&self, fill: [f32; 4]) -> [f32; 4] {
        let Some(options) = &self.circle else {
            return fill;
        };
        let Some(stroke) = &options.stroke_color else {
            return fill;
        };
        let stroke = stroke
            .evaluate_for(self.props(), options.zoom)
            .map_or([0.0, 0.0, 0.0, 1.0], |color| color.to_array());
        pack_circle_colors(fill, stroke)
    }

    pub(super) fn emit_circle(&mut self, x: f32, y: f32) {
        let Some(options) = &self.circle else {
            return;
        };
        let extent = EXTENT as f32;
        if !(0.0..extent).contains(&x) || !(0.0..extent).contains(&y) {
            return;
        }
        let props = self.props();
        let radius = options
            .radius
            .evaluate_for(props, options.zoom)
            .unwrap_or(options.radius_default)
            .max(0.0);
        // A radius that varies by feature and by zoom is stored for the tile's zoom and the one
        // above it, which the shader blends by the fractional zoom, as GL JS does.
        let radius_next = if options.radius.is_feature_constant() {
            radius
        } else {
            options
                .radius
                .evaluate_for(props, options.zoom + 1.0)
                .unwrap_or(options.radius_default)
                .max(0.0)
        };
        let stroke_width = options
            .stroke_width
            .evaluate_for(props, options.zoom)
            .unwrap_or(options.stroke_width_default)
            .max(0.0);

        // Negative values tell the shader to keep the layer's own blur and stroke opacity.
        let per_feature = |property: &Option<StyleProperty<f32>>| {
            property
                .as_ref()
                .and_then(|property| property.evaluate_for(props, options.zoom))
                .map_or(-1.0, |value| value.max(0.0))
        };
        let (blur, stroke_opacity) = (
            per_feature(&options.blur),
            per_feature(&options.stroke_opacity),
        );
        let base = self.buffer.vertices.len() as u32;
        let push = |code: f32, buffer: &mut Vec<ShaderVertex>| {
            let mut vertex = ShaderVertex::new([x, y], [radius, stroke_width]);
            vertex.distance = blur;
            vertex.edge_distance = radius_next;
            vertex.corner_code = code + stroke_opacity_fraction(stroke_opacity);
            buffer.push(vertex);
        };
        if options.grid {
            for ey in CIRCLE_GRID_STEPS {
                for ex in CIRCLE_GRID_STEPS {
                    push(1.0 + (ex * 8 + ey) as f32, &mut self.buffer.vertices);
                }
            }
            for row in 0..3_u32 {
                for column in 0..3_u32 {
                    let lower = base + row * 4 + column;
                    let upper = lower + 4;
                    for offset in [lower, lower + 1, upper + 1, lower, upper + 1, upper] {
                        self.buffer.indices.push(I::from(VertexId(offset)));
                    }
                }
            }
        } else {
            for _ in 0..4 {
                push(0.0, &mut self.buffer.vertices);
            }
            for offset in CIRCLE_QUAD_INDICES {
                self.buffer.indices.push(I::from(VertexId(base + offset)));
            }
        }
    }
}

#[cfg(test)]
mod tests;
