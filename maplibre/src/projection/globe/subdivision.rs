//! Geometry subdivision needed to approximate curved globe features.

use lyon::tessellation::{VertexBuffers, VertexId};
use rustc_hash::FxHashMap as HashMap;
use thiserror::Error;

use crate::{coords::EXTENT_UINT, render::ShaderVertex};

const MAX_GENERATED_VERTICES_PER_SEGMENT: usize = 16_384;
const MAX_CELLS_PER_TRIANGLE: usize = 65_536;
const NORTH_POLE_Y: f32 = i16::MIN as f32;
const SOUTH_POLE_Y: f32 = i16::MAX as f32;

mod poles;

/// Controls fill-mesh subdivision and globe-only edge geometry.
#[derive(Clone, Copy, Debug)]
pub struct FillSubdivisionOptions {
    /// Number of grid cells along each tile axis.
    pub granularity: u32,
    /// Rejects buffered geometry beyond the z0 antimeridian edges.
    pub clip_x_to_tile: bool,
    /// Extends fill edges at y=0 to the north pole.
    pub extend_to_north_pole: bool,
    /// Extends fill edges at y=extent to the south pole.
    pub extend_to_south_pole: bool,
}

/// Failure while subdividing untrusted tile geometry.
#[derive(Clone, Copy, Debug, Error, PartialEq)]
pub enum SubdivisionError {
    /// A line would generate an unreasonable number of vertices.
    #[error("globe line subdivision exceeds {limit} generated vertices")]
    LineVertexLimit {
        /// Maximum allowed generated vertices.
        limit: usize,
    },
    /// A triangle overlaps an unreasonable number of subdivision cells.
    #[error("globe fill subdivision exceeds {limit} cells for one triangle")]
    TriangleCellLimit {
        /// Maximum allowed intersected cells.
        limit: usize,
    },
    /// A tessellation index does not address an existing vertex.
    #[error("globe subdivision index {index} is outside {vertex_count} vertices")]
    InvalidIndex {
        /// Invalid vertex index.
        index: usize,
        /// Number of available vertices.
        vertex_count: usize,
    },
}

/// Returns the GL JS subdivision policy for a canonical zoom level.
pub fn granularity_for_zoom(base: u32, minimum: u32, zoom: u8) -> u32 {
    let scaled = base.checked_shr(u32::from(zoom)).unwrap_or(0);
    scaled.max(minimum).max(1)
}

/// Inserts every grid-axis intersection along one line segment.
pub fn subdivide_line_segment(
    start: [f32; 2],
    end: [f32; 2],
    granularity: u32,
) -> Result<Vec<[f32; 2]>, SubdivisionError> {
    subdivide_line_segment_on_grid(start, end, granularity, false)
}

/// Inserts every grid-axis intersection along one line segment and, with `diagonals`, every
/// crossing of a cell diagonal from its top-left to its bottom-right corner as well, where a
/// terrain mesh splits its cells into triangles.
pub fn subdivide_line_segment_on_grid(
    start: [f32; 2],
    end: [f32; 2],
    granularity: u32,
    diagonals: bool,
) -> Result<Vec<[f32; 2]>, SubdivisionError> {
    if granularity < 2 || start == end {
        return Ok(vec![end]);
    }
    if !start[0].is_finite() || !start[1].is_finite() || !end[0].is_finite() || !end[1].is_finite()
    {
        return Err(SubdivisionError::LineVertexLimit {
            limit: MAX_GENERATED_VERTICES_PER_SEGMENT,
        });
    }
    let cell_size = (EXTENT_UINT / granularity).max(1) as f32;
    let delta = [end[0] - start[0], end[1] - start[1]];
    let mut crossings = Vec::new();
    collect_axis_crossings(start[0], end[0], cell_size, &mut crossings)?;
    collect_axis_crossings(start[1], end[1], cell_size, &mut crossings)?;
    if diagonals {
        // The diagonals are the lines on which x - y is a whole number of cells.
        collect_axis_crossings(
            start[0] - start[1],
            end[0] - end[1],
            cell_size,
            &mut crossings,
        )?;
    }
    crossings.sort_by(f32::total_cmp);
    crossings.dedup_by(|left, right| (*left - *right).abs() <= f32::EPSILON);
    if crossings.len() > MAX_GENERATED_VERTICES_PER_SEGMENT {
        return Err(SubdivisionError::LineVertexLimit {
            limit: MAX_GENERATED_VERTICES_PER_SEGMENT,
        });
    }
    let mut points = crossings
        .into_iter()
        .filter(|fraction| *fraction > 0.0 && *fraction < 1.0)
        .map(|fraction| {
            [
                start[0] + delta[0] * fraction,
                start[1] + delta[1] * fraction,
            ]
        })
        .collect::<Vec<_>>();
    points.dedup();
    if points.last().copied() != Some(end) {
        points.push(end);
    }
    Ok(points)
}

/// Clips newly tessellated triangles to the regular subdivision grid.
pub fn subdivide_triangles<I>(
    buffer: &mut VertexBuffers<ShaderVertex, I>,
    index_start: usize,
    options: FillSubdivisionOptions,
) -> Result<(), SubdivisionError>
where
    I: Copy + Into<u32> + From<VertexId>,
{
    subdivide_triangles_from(buffer, buffer.vertices.len(), index_start, options)
}

/// Clips newly tessellated triangles added starting at `(vertex_start, index_start)` to the
/// regular subdivision grid, discarding the un-subdivided source vertices when every input index
/// lies at or above `vertex_start`.
pub fn subdivide_triangles_from<I>(
    buffer: &mut VertexBuffers<ShaderVertex, I>,
    vertex_start: usize,
    index_start: usize,
    options: FillSubdivisionOptions,
) -> Result<(), SubdivisionError>
where
    I: Copy + Into<u32> + From<VertexId>,
{
    if options.granularity < 2 {
        return Ok(());
    }
    let Some(slice) = buffer.indices.get(index_start..) else {
        return Ok(());
    };
    let can_truncate = slice
        .iter()
        .all(|&index| (index.into() as usize) >= vertex_start);
    let mut input_triangles = Vec::with_capacity(slice.len() / 3);
    for triangle in slice.as_chunks::<3>().0 {
        input_triangles.push(triangle_points(&buffer.vertices, triangle)?);
    }
    buffer.indices.truncate(index_start);
    if can_truncate {
        buffer.vertices.truncate(vertex_start);
    }
    let mut vertices =
        HashMap::with_capacity_and_hasher(input_triangles.len() * 3, Default::default());
    for points in input_triangles {
        subdivide_triangle(
            buffer,
            points,
            options.granularity,
            options.clip_x_to_tile,
            &mut vertices,
        )?;
    }
    poles::append_pole_quads(buffer, index_start, options, &mut vertices);
    Ok(())
}

/// Keeps a tessellated point strictly between the pole sentinels, even once rounded to a vertex,
/// so only `append_pole_quads` makes vertices the shader draws at a pole. Only the copy read for
/// clipping is changed: the buffer holds earlier polygons' pole vertices, which must stay exact.
fn off_pole_sentinels(point: [f32; 2]) -> [f32; 2] {
    [
        point[0],
        point[1].clamp(NORTH_POLE_Y + 1.0, SOUTH_POLE_Y - 1.0),
    ]
}

fn collect_axis_crossings(
    start: f32,
    end: f32,
    cell_size: f32,
    output: &mut Vec<f32>,
) -> Result<(), SubdivisionError> {
    if !start.is_finite() || !end.is_finite() || !cell_size.is_finite() || cell_size <= 0.0 {
        return Err(SubdivisionError::LineVertexLimit {
            limit: MAX_GENERATED_VERTICES_PER_SEGMENT,
        });
    }
    if start == end {
        return Ok(());
    }
    let low = start.min(end);
    let high = start.max(end);
    let first_f = (low / cell_size).floor();
    let last_f = (high / cell_size).ceil();
    if !first_f.is_finite()
        || !last_f.is_finite()
        || first_f < i64::MIN as f32 / 2.0
        || last_f > i64::MAX as f32 / 2.0
    {
        return Err(SubdivisionError::LineVertexLimit {
            limit: MAX_GENERATED_VERTICES_PER_SEGMENT,
        });
    }
    let first = first_f as i64 + 1;
    let last = last_f as i64;
    if last.saturating_sub(first) > MAX_GENERATED_VERTICES_PER_SEGMENT as i64 {
        return Err(SubdivisionError::LineVertexLimit {
            limit: MAX_GENERATED_VERTICES_PER_SEGMENT,
        });
    }
    for axis in first..last {
        let coordinate = axis as f32 * cell_size;
        output.push((coordinate - start) / (end - start));
    }
    Ok(())
}

fn triangle_points<I: Copy + Into<u32>>(
    vertices: &[ShaderVertex],
    indices: &[I],
) -> Result<[[f32; 2]; 3], SubdivisionError> {
    let mut points = [[0.0; 2]; 3];
    for (slot, index) in points.iter_mut().zip(indices) {
        let index = (*index).into() as usize;
        let vertex = vertices.get(index).ok_or(SubdivisionError::InvalidIndex {
            index,
            vertex_count: vertices.len(),
        })?;
        *slot = off_pole_sentinels(vertex.position);
    }
    Ok(points)
}

const MAX_CLIPPED_POLYGON_VERTICES: usize = 16;

/// Stack-allocated polygon buffer avoiding heap allocations during Sutherland-Hodgman clipping.
#[derive(Clone, Copy, Debug)]
pub(crate) struct InlinePolygon {
    vertices: [[f32; 2]; MAX_CLIPPED_POLYGON_VERTICES],
    len: usize,
}

impl InlinePolygon {
    #[inline]
    pub(crate) fn empty() -> Self {
        Self {
            vertices: [[0.0; 2]; MAX_CLIPPED_POLYGON_VERTICES],
            len: 0,
        }
    }

    #[inline]
    pub(crate) fn from_triangle(triangle: [[f32; 2]; 3]) -> Self {
        let mut vertices = [[0.0; 2]; MAX_CLIPPED_POLYGON_VERTICES];
        vertices[0] = triangle[0];
        vertices[1] = triangle[1];
        vertices[2] = triangle[2];
        Self { vertices, len: 3 }
    }

    #[inline]
    pub(crate) fn as_slice(&self) -> &[[f32; 2]] {
        self.vertices.get(..self.len).unwrap_or(&self.vertices)
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub(crate) fn push(&mut self, point: [f32; 2]) {
        if let Some(slot) = self.vertices.get_mut(self.len) {
            *slot = point;
            self.len += 1;
        }
    }

    #[inline]
    pub(crate) fn clear(&mut self) {
        self.len = 0;
    }
}

fn subdivide_triangle<I>(
    buffer: &mut VertexBuffers<ShaderVertex, I>,
    triangle: [[f32; 2]; 3],
    granularity: u32,
    clip_x_to_tile: bool,
    cache: &mut HashMap<(i32, i32), usize>,
) -> Result<(), SubdivisionError>
where
    I: From<VertexId>,
{
    if !triangle.iter().flatten().all(|c| c.is_finite()) {
        return Err(SubdivisionError::TriangleCellLimit {
            limit: MAX_CELLS_PER_TRIANGLE,
        });
    }
    let cell_size = (EXTENT_UINT / granularity).max(1) as f32;
    let bounds = triangle_bounds(triangle, cell_size);
    if bounds.1 <= bounds.0 || bounds.3 <= bounds.2 {
        return Ok(());
    }
    if bounds.1 - bounds.0 == 1 && bounds.3 - bounds.2 == 1 {
        append_polygon(buffer, &triangle, clip_x_to_tile, cache);
        return Ok(());
    }
    let cell_count = usize::try_from(bounds.1.saturating_sub(bounds.0))
        .ok()
        .and_then(|w| {
            usize::try_from(bounds.3.saturating_sub(bounds.2))
                .ok()
                .and_then(|h| w.checked_mul(h))
        })
        .ok_or(SubdivisionError::TriangleCellLimit {
            limit: MAX_CELLS_PER_TRIANGLE,
        })?;
    if cell_count > MAX_CELLS_PER_TRIANGLE {
        return Err(SubdivisionError::TriangleCellLimit {
            limit: MAX_CELLS_PER_TRIANGLE,
        });
    }
    let mut poly_b = InlinePolygon::empty();
    for y in bounds.2..bounds.3 {
        for x in bounds.0..bounds.1 {
            let mut poly_a = InlinePolygon::from_triangle(triangle);
            clip_axis_into(&poly_a, &mut poly_b, 0, x as f32 * cell_size, true);
            if poly_b.len() < 3 {
                continue;
            }
            clip_axis_into(&poly_b, &mut poly_a, 0, (x + 1) as f32 * cell_size, false);
            if poly_a.len() < 3 {
                continue;
            }
            clip_axis_into(&poly_a, &mut poly_b, 1, y as f32 * cell_size, true);
            if poly_b.len() < 3 {
                continue;
            }
            clip_axis_into(&poly_b, &mut poly_a, 1, (y + 1) as f32 * cell_size, false);
            append_polygon(buffer, poly_a.as_slice(), clip_x_to_tile, cache);
        }
    }
    Ok(())
}

fn triangle_bounds(triangle: [[f32; 2]; 3], cell_size: f32) -> (i64, i64, i64, i64) {
    let min_x = triangle
        .iter()
        .map(|point| point[0])
        .fold(f32::INFINITY, f32::min);
    let max_x = triangle
        .iter()
        .map(|point| point[0])
        .fold(f32::NEG_INFINITY, f32::max);
    let min_y = triangle
        .iter()
        .map(|point| point[1])
        .fold(f32::INFINITY, f32::min);
    let max_y = triangle
        .iter()
        .map(|point| point[1])
        .fold(f32::NEG_INFINITY, f32::max);
    (
        (min_x / cell_size).floor() as i64,
        (max_x / cell_size).ceil() as i64,
        (min_y / cell_size).floor() as i64,
        (max_y / cell_size).ceil() as i64,
    )
}

pub(crate) fn clip_axis_into(
    input: &InlinePolygon,
    output: &mut InlinePolygon,
    axis: usize,
    boundary: f32,
    keep_greater: bool,
) {
    output.clear();
    let slice = input.as_slice();
    let Some(mut previous) = slice.last().copied() else {
        return;
    };
    for &current in slice {
        let prev_val = previous.get(axis).copied().unwrap_or(0.0);
        let curr_val = current.get(axis).copied().unwrap_or(0.0);
        let previous_inside = (prev_val >= boundary) == keep_greater;
        let current_inside = (curr_val >= boundary) == keep_greater;
        if previous_inside != current_inside {
            output.push(axis_intersection(previous, current, axis, boundary));
        }
        if current_inside {
            output.push(current);
        }
        previous = current;
    }
}

fn axis_intersection(start: [f32; 2], end: [f32; 2], axis: usize, boundary: f32) -> [f32; 2] {
    let start_val = start.get(axis).copied().unwrap_or(0.0);
    let end_val = end.get(axis).copied().unwrap_or(0.0);
    let (a, b, a_val, b_val) = if start_val <= end_val {
        (start, end, start_val, end_val)
    } else {
        (end, start, end_val, start_val)
    };
    let fraction = (boundary - a_val) / (b_val - a_val);
    let mut point = [
        (a[0] + (b[0] - a[0]) * fraction).round(),
        (a[1] + (b[1] - a[1]) * fraction).round(),
    ];
    if let Some(coord) = point.get_mut(axis) {
        *coord = boundary;
    }
    point
}

fn append_polygon<I: From<VertexId>>(
    buffer: &mut VertexBuffers<ShaderVertex, I>,
    polygon: &[[f32; 2]],
    clip_x_to_tile: bool,
    cache: &mut HashMap<(i32, i32), usize>,
) {
    if polygon.len() < 3
        || (clip_x_to_tile
            && polygon
                .iter()
                .any(|point| point[0] < 0.0 || point[0] > EXTENT_UINT as f32))
    {
        return;
    }
    let Some(&first_pt) = polygon.first() else {
        return;
    };
    let first = vertex_index(buffer, first_pt, cache);
    for pair in polygon.get(1..).unwrap_or(&[]).windows(2) {
        let &[p1, p2] = pair else {
            continue;
        };
        let second = vertex_index(buffer, p1, cache);
        let third = vertex_index(buffer, p2, cache);
        if first != second && second != third && third != first {
            buffer
                .indices
                .extend([first, second, third].map(|index| I::from(VertexId::from_usize(index))));
        }
    }
}

pub(super) fn vertex_index<I>(
    buffer: &mut VertexBuffers<ShaderVertex, I>,
    point: [f32; 2],
    cache: &mut HashMap<(i32, i32), usize>,
) -> usize {
    let point = [point[0].round(), point[1].round()];
    let key = (point[0] as i32, point[1] as i32);
    if let Some(index) = cache.get(&key) {
        return *index;
    }
    let index = buffer.vertices.len();
    buffer.vertices.push(ShaderVertex::new(point, [0.0, 0.0]));
    cache.insert(key, index);
    index
}

#[cfg(test)]
mod tests;
