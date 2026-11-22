//! The one-pixel outline of a filled polygon, drawn as a feature after its fill.

use lyon::tessellation::{StrokeVertex, StrokeVertexConstructor};

use super::{VertexConstructor, ZeroTessellator};
use crate::render::{shaders::GpuVertexKind, ShaderVertex};

/// Only the path and its normals matter to the outline; the fill shader moves each vertex
/// along its normal by a width that follows the view's zoom.
pub(super) const OUTLINE_WIDTH: f32 = 1.0;
/// Half the outline's width in screen pixels.
const HALF_WIDTH_PIXELS: f32 = 0.5;
/// Tile units the fill shader moves a vertex along a unit normal, per tile unit of zoom factor,
/// as a multiple of one screen pixel (eight tile units).
const NORMAL_UNITS_PER_PIXEL: f32 = 8.0 / 3.0;
/// Miter limit passed to `lyon` (`|normal| <= 2.0 * OUTLINE_MITER_LIMIT == 2.0`), matching
/// MapLibre GL JS's `2x` half-width threshold while keeping 90° corners crisp.
pub(super) const OUTLINE_MITER_LIMIT: f32 = 1.0;
/// Maximum scaled normal magnitude permitted on a `fill-outline-color` stroke vertex.
pub(super) const MAX_OUTLINE_NORMAL: f32 =
    2.0 * OUTLINE_MITER_LIMIT * HALF_WIDTH_PIXELS * NORMAL_UNITS_PER_PIXEL;

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
    /// Compacts one tessellated polygon outline into 16-byte [`GpuShaderVertex`] storage so
    /// multi-polygon buildings never retain 56-byte [`ShaderVertex`] scratch across polygons.
    pub(super) fn flush_polygon_outline_prepack(&mut self, index_start: usize) {
        if self.streaming_prepack.is_none() || self.outline.vertices.is_empty() {
            return;
        }
        let base = self.outline_prepack.len();
        if base > 0
            && let Some(indices) = self.outline.indices.get_mut(index_start..)
        {
            for idx in indices {
                *idx = I::from(lyon::tessellation::VertexId::from_usize(
                    base + (*idx).into() as usize,
                ));
            }
        }
        self.outline_prepack.reserve(self.outline.vertices.len());
        for vertex in &self.outline.vertices {
            self.outline_prepack
                .push(GpuVertexKind::Fill.encode(vertex));
        }
        self.outline.vertices.clear();
    }

    /// Draws the pending outline as a feature of its own, after the fill it belongs to.
    pub(super) fn append_outline(&mut self) -> bool {
        if self.outline.vertices.is_empty() && self.outline_prepack.is_empty() {
            self.outline.indices.clear();
            return false;
        }
        let Some(property) = &self.outline_property else {
            self.clear_outline_buffers();
            return false;
        };
        let evaluated = if self.omit_feature_colors {
            None
        } else {
            let Some(colour) = property.evaluate_for(self.props(), self.zoom) else {
                self.clear_outline_buffers();
                return false;
            };
            Some(colour)
        };
        self.flush_outline_geometry();
        self.update_feature_indices();
        if self.record_outline_entries {
            self.outline_entries.push(true);
        }
        if let Some(colour) = evaluated {
            let opacity = self
                .feature_opacity
                .as_ref()
                .map_or(1.0, |(opacity, zoom)| {
                    opacity
                        .evaluate_for(self.props(), *zoom)
                        .unwrap_or(1.0)
                        .clamp(0.0, 1.0)
                });
            self.feature_colors
                .push([colour.r, colour.g, colour.b, colour.a * opacity]);
        }
        true
    }

    fn clear_outline_buffers(&mut self) {
        self.outline.vertices.clear();
        self.outline.indices.clear();
        self.outline_prepack.clear();
    }

    fn flush_outline_geometry(&mut self) {
        let index_start = self.buffer.indices.len();
        let using_prepack = self.streaming_prepack.is_some() && !self.outline_prepack.is_empty();
        if using_prepack {
            self.flush_streaming_prepack(index_start);
            let base = self.total_vertex_count();
            self.push_outline_indices(base);
            if let Some(prepack) = &mut self.streaming_prepack {
                prepack.push_encoded(&self.outline_prepack);
            }
            self.outline_prepack.clear();
            if self.outline_prepack.capacity() > 16_384 {
                self.outline_prepack = Vec::new();
            }
        } else {
            let base = self.buffer.vertices.len();
            self.buffer
                .vertices
                .extend_from_slice(&self.outline.vertices);
            self.push_outline_indices(base);
            self.outline.vertices.clear();
            if self.outline.vertices.capacity() > 16_384 {
                self.outline.vertices = Vec::new();
            }
            self.flush_streaming_prepack(index_start);
        }
        if self.outline.indices.capacity() > 32_768 {
            self.outline.indices = Vec::new();
        }
    }

    fn push_outline_indices(&mut self, base: usize) {
        if base == 0 {
            self.buffer.indices.extend_from_slice(&self.outline.indices);
        } else {
            self.buffer
                .indices
                .extend(self.outline.indices.iter().map(|&index| {
                    I::from(lyon::tessellation::VertexId::from_usize(
                        base + index.into() as usize,
                    ))
                }));
        }
        self.outline.indices.clear();
    }
}
