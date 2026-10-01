//! The one-pixel outline of a filled polygon, drawn as a feature after its fill.

use lyon::tessellation::{StrokeVertex, StrokeVertexConstructor, VertexBuffers};

use super::{VertexConstructor, ZeroTessellator};
use crate::render::ShaderVertex;

/// Outline width in tile units: about one pixel of a 512 px tile.
pub(super) const OUTLINE_WIDTH: f32 = 8.0;

impl StrokeVertexConstructor<ShaderVertex> for VertexConstructor {
    fn new_vertex(&mut self, vertex: StrokeVertex) -> ShaderVertex {
        ShaderVertex::new(vertex.position().to_array(), [0.0, 0.0])
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
