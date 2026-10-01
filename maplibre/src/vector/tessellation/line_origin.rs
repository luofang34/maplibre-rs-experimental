//! Where a stroke's path distance starts: where the line first enters the tile's buffer, as in
//! a source that clips its lines to each tile, so a dash pattern begins at the same place for
//! a line that starts far outside the tile and stays precise where the distances would be huge.
use lyon::{
    path::{Event, Path},
    tessellation::{StrokeVertex, StrokeVertexConstructor, VertexSource},
};

use super::line_style::PackedLine;
use crate::render::ShaderVertex;

/// Tile units either side of a tile that a line clipped for it reaches: the quarter of a tile
/// that GL JS keeps when it clips the lines of a GeoJSON source.
const BUFFER: f32 = 1024.0;
const EXTENT: f32 = 4096.0;

/// The distance along its sub-path at which each endpoint's line enters the buffered tile.
pub fn entry_distances(path: &Path) -> Vec<f32> {
    let mut origins = Vec::new();
    let mut sub_path_start = 0;
    let mut travelled = 0.0;
    let mut entered = false;
    for event in path.iter() {
        match event {
            Event::Begin { at } => {
                sub_path_start = origins.len();
                travelled = 0.0;
                entered = inside(at);
                origins.push(0.0);
            }
            Event::Line { from, to } => {
                let length = (to - from).length();
                if !entered {
                    if let Some(at) = entry_parameter(from, to) {
                        let origin = travelled + at * length;
                        origins[sub_path_start..].fill(origin);
                        entered = true;
                    }
                }
                travelled += length;
                origins.push(origins[sub_path_start]);
            }
            _ => {}
        }
    }
    origins
}

fn inside(point: lyon::math::Point) -> bool {
    (-BUFFER..=EXTENT + BUFFER).contains(&point.x) && (-BUFFER..=EXTENT + BUFFER).contains(&point.y)
}

/// The fraction of the segment at which it first lies in the buffered tile, if it does.
fn entry_parameter(from: lyon::math::Point, to: lyon::math::Point) -> Option<f32> {
    let (low, high) = (-BUFFER, EXTENT + BUFFER);
    let (mut enter, mut leave) = (0.0_f32, 1.0_f32);
    for (start, delta) in [(from.x, to.x - from.x), (from.y, to.y - from.y)] {
        if delta.abs() < f32::EPSILON {
            if start < low || start > high {
                return None;
            }
            continue;
        }
        let (a, b) = ((low - start) / delta, (high - start) / delta);
        enter = enter.max(a.min(b));
        leave = leave.min(a.max(b));
    }
    (enter <= leave).then_some(enter)
}

/// Builds stroke vertices whose distance counts from where their line enters the tile.
pub struct StrokeOrigins {
    /// Entry distance of each path endpoint's sub-path, from [`entry_distances`].
    pub origins: Vec<f32>,
    /// What the feature's line style adds to each vertex.
    pub packed: PackedLine,
}

impl StrokeVertexConstructor<ShaderVertex> for StrokeOrigins {
    fn new_vertex(&mut self, vertex: StrokeVertex) -> ShaderVertex {
        let mut output = ShaderVertex::new(
            vertex.position_on_path().to_array(),
            vertex.normal().to_array(),
        );
        let endpoint = match vertex.source() {
            VertexSource::Endpoint { id } => id,
            VertexSource::Edge { from, .. } => from,
        };
        let origin = self
            .origins
            .get(endpoint.to_usize())
            .copied()
            .unwrap_or(0.0);
        let distance = vertex.advancement() - origin;
        // GL JS stores a vertex's distance in whole tile units, so the phase of a dash or pattern
        // drifts from one line to the next by up to one unit; a gradient keeps its fraction.
        output.distance = if self.origins.is_empty() {
            distance
        } else {
            distance.floor()
        };
        // A stroke has no elevation; the slot instead says which side of the line the vertex is
        // on, which a pattern needs to draw the image across the line the right way up, and
        // carries the gap width and blur.
        output.elevation = self
            .packed
            .stroke_elevation(matches!(vertex.side(), lyon::tessellation::Side::Negative));
        output.edge_distance = self.packed.style;
        output
    }
}

#[cfg(test)]
mod tests {
    use lyon::math::point;

    use super::*;

    #[test]
    fn a_line_from_far_outside_counts_its_distance_from_where_it_reaches_the_tile() {
        let mut builder = Path::builder();
        builder.begin(point(-10_000.0, 100.0));
        builder.line_to(point(10_000.0, 100.0));
        builder.end(false);
        let origins = entry_distances(&builder.build());
        assert_eq!(origins.len(), 2);
        assert!((origins[0] - (10_000.0 - BUFFER)).abs() < 0.01);
        assert_eq!(origins[0], origins[1]);
    }

    #[test]
    fn a_line_that_starts_inside_keeps_its_own_start() {
        let mut builder = Path::builder();
        builder.begin(point(10.0, 10.0));
        builder.line_to(point(9_000.0, 10.0));
        builder.end(false);
        assert_eq!(entry_distances(&builder.build()), [0.0, 0.0]);
    }
}
