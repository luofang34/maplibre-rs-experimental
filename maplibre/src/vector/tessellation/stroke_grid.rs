//! Strokes split along a grid of tile cells: the globe's, so a line follows the curve of the
//! sphere, and the terrain mesh's, so a line drawn over the terrain follows its faces.
use geozero::error::{GeozeroError, Result as GeoResult};
use lyon::geom;

use super::{MaxIndex, ZeroTessellator};
use crate::{
    projection::globe::subdivision::subdivide_line_segment_on_grid,
    style::{layer::StyleLayer, line_stroke::LineStroke},
    terrain::mesh::TERRAIN_MESH_SIZE,
};

impl<I> ZeroTessellator<I>
where
    I: std::ops::Add + From<lyon::tessellation::VertexId> + MaxIndex + Copy + Into<u32>,
{
    /// Strokes features as `layer` lays them out: with its cap and join and, for a line drawn
    /// over the terrain surface, split at every edge of the triangles of a terrain mesh over
    /// this tile or any coarser one, so each straight piece lies on one planar face of the
    /// surface the shader lifts it onto.
    pub fn set_layer_strokes(&mut self, layer: &StyleLayer) {
        self.stroke = LineStroke::of_layer(layer);
        self.join_property = LineStroke::join_property(layer);
        if crate::vector::structures::is_overlay(layer) {
            self.subdivision_granularity = self.subdivision_granularity.max(TERRAIN_MESH_SIZE);
            self.terrain_grid = true;
        }
    }

    pub(super) fn append_coordinate(&mut self, coordinate: [f32; 2]) -> GeoResult<()> {
        if let Some(previous) = self.contour_last {
            let points = subdivide_line_segment_on_grid(
                previous,
                coordinate,
                self.subdivision_granularity,
                self.terrain_grid,
            )
            .map_err(|error| GeozeroError::Geometry(error.to_string()))?;
            for point in points {
                self.path_builder
                    .borrow_mut()
                    .line_to(geom::point(point[0], point[1]));
            }
        } else {
            self.path_builder
                .borrow_mut()
                .begin(geom::point(coordinate[0], coordinate[1]));
            self.contour_start = Some(coordinate);
            self.path_open = true;
        }
        self.contour_last = Some(coordinate);
        Ok(())
    }

    pub(super) fn subdivide_closing_segment(&mut self) -> GeoResult<()> {
        let (Some(start), Some(last)) = (self.contour_start, self.contour_last) else {
            return Ok(());
        };
        let mut points = subdivide_line_segment_on_grid(
            last,
            start,
            self.subdivision_granularity,
            self.terrain_grid,
        )
        .map_err(|error| GeozeroError::Geometry(error.to_string()))?;
        points.pop();
        for point in points {
            self.path_builder
                .borrow_mut()
                .line_to(geom::point(point[0], point[1]));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
