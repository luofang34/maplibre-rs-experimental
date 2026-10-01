//! The one-pixel outline of a filled polygon, drawn as a feature after its fill.

use lyon::tessellation::{StrokeVertex, StrokeVertexConstructor, VertexBuffers};

use super::{VertexConstructor, ZeroTessellator};
use crate::render::ShaderVertex;

/// Only the path and its normals matter to the outline; the fill shader moves each vertex
/// along its normal by a width that follows the view's zoom.
pub(super) const OUTLINE_WIDTH: f32 = 1.0;
/// Half the outline's width in screen pixels.
const HALF_WIDTH_PIXELS: f32 = 0.5;
/// Tile units the fill shader moves a vertex along a unit normal, per tile unit of zoom factor,
/// as a multiple of one screen pixel (eight tile units).
const NORMAL_UNITS_PER_PIXEL: f32 = 8.0 / 3.0;

impl StrokeVertexConstructor<ShaderVertex> for VertexConstructor {
    fn new_vertex(&mut self, vertex: StrokeVertex) -> ShaderVertex {
        let normal = vertex.normal() * (HALF_WIDTH_PIXELS * NORMAL_UNITS_PER_PIXEL);
        ShaderVertex::new(vertex.position_on_path().to_array(), normal.to_array())
    }
}

impl<I> ZeroTessellator<I>
where
    I: std::ops::Add
        + From<lyon::tessellation::VertexId>
        + lyon::tessellation::geometry_builder::MaxIndex
        + Copy
        + Into<u32>,
{
    /// Draws the pending outline as a feature of its own, after the fill it belongs to.
    pub(super) fn append_outline(&mut self) {
        let outline = std::mem::replace(&mut self.outline, VertexBuffers::new());
        let Some(property) = &self.outline_property else {
            return;
        };
        let Some(colour) = property.evaluate_for(&self.feature_properties, self.zoom) else {
            return;
        };
        if outline.vertices.is_empty() {
            return;
        }
        let base = self.buffer.vertices.len();
        self.buffer.vertices.extend(outline.vertices);
        self.buffer
            .indices
            .extend(outline.indices.into_iter().map(|index| {
                I::from(lyon::tessellation::VertexId::from_usize(
                    base + index.into() as usize,
                ))
            }));
        self.update_feature_indices();
        let opacity = self
            .feature_opacity
            .as_ref()
            .map_or(1.0, |(opacity, zoom)| {
                opacity
                    .evaluate_for(&self.feature_properties, *zoom)
                    .unwrap_or(1.0)
                    .clamp(0.0, 1.0)
            });
        self.feature_colors.push([
            colour.r as f32,
            colour.g as f32,
            colour.b as f32,
            colour.a as f32 * opacity,
        ]);
    }
}
