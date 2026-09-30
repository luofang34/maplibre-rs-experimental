//! Feeds the geometry of a feature into the tessellator's path or circle builders.
use geozero::GeomProcessor;
use lyon::tessellation::geometry_builder::MaxIndex;

use super::{GeoResult, ZeroTessellator};

impl<I> GeomProcessor for ZeroTessellator<I>
where
    I: std::ops::Add + From<lyon::tessellation::VertexId> + MaxIndex + Copy + Into<u32>,
{
    fn xy(&mut self, x: f64, y: f64, _idx: usize) -> GeoResult<()> {
        let scale = self.coordinate_scale;
        let coordinate = [(x * scale) as f32, (y * scale) as f32];
        if self.circle.is_some() {
            self.emit_circle(coordinate[0], coordinate[1]);
        } else if !self.is_point {
            self.append_coordinate(coordinate)?;
        }
        Ok(())
    }

    fn point_begin(&mut self, _idx: usize) -> GeoResult<()> {
        self.is_point = true;
        Ok(())
    }

    fn point_end(&mut self, _idx: usize) -> GeoResult<()> {
        self.is_point = false;
        Ok(())
    }

    fn multipoint_begin(&mut self, _size: usize, _idx: usize) -> GeoResult<()> {
        Ok(())
    }

    fn multipoint_end(&mut self, _idx: usize) -> GeoResult<()> {
        Ok(())
    }

    fn linestring_begin(&mut self, _tagged: bool, _size: usize, _idx: usize) -> GeoResult<()> {
        Ok(())
    }

    fn linestring_end(&mut self, tagged: bool, _idx: usize) -> GeoResult<()> {
        if self.circle.is_some() {
            return Ok(());
        }
        self.end(false)?;

        if tagged {
            self.tessellate_strokes()?;
        }
        Ok(())
    }

    fn multilinestring_begin(&mut self, _size: usize, _idx: usize) -> GeoResult<()> {
        Ok(())
    }

    fn multilinestring_end(&mut self, _idx: usize) -> GeoResult<()> {
        if self.circle.is_some() {
            return Ok(());
        }
        self.tessellate_strokes()?;
        Ok(())
    }

    fn polygon_begin(&mut self, _tagged: bool, _size: usize, _idx: usize) -> GeoResult<()> {
        Ok(())
    }

    fn polygon_end(&mut self, tagged: bool, _idx: usize) -> GeoResult<()> {
        if self.circle.is_some() {
            return Ok(());
        }
        self.end(true)?;
        if tagged {
            if self.is_line_layer {
                self.tessellate_strokes()?;
            } else {
                self.tessellate_fill()?;
            }
        }
        Ok(())
    }

    fn multipolygon_begin(&mut self, _size: usize, _idx: usize) -> GeoResult<()> {
        Ok(())
    }

    fn multipolygon_end(&mut self, _idx: usize) -> GeoResult<()> {
        if self.circle.is_some() {
            return Ok(());
        }
        if self.is_line_layer {
            self.tessellate_strokes()?;
        } else {
            self.tessellate_fill()?;
        }
        Ok(())
    }
}
