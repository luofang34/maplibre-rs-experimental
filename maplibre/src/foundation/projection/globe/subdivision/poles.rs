//! Extrudes clipped polar tile boundaries of subdivided fill polygons to the globe poles.

use std::ops::Range;

use lyon::tessellation::{VertexBuffers, VertexId};
use rustc_hash::FxHashMap as HashMap;

use super::{vertex_index, FillSubdivisionOptions, NORTH_POLE_Y, SOUTH_POLE_Y};
use crate::{coords::EXTENT_UINT, projection::ShaderVertex};

pub(super) fn append_pole_quads<I>(
    buffer: &mut VertexBuffers<ShaderVertex, I>,
    index_start: usize,
    options: FillSubdivisionOptions,
    cache: &mut HashMap<(i32, i32), usize>,
) where
    I: Copy + Into<u32> + From<VertexId>,
{
    if !options.extend_to_north_pole && !options.extend_to_south_pole {
        return;
    }
    let input_end = buffer.indices.len();
    if index_start >= input_end {
        return;
    }

    let (min_y, max_y, edge_counts) = collect_edge_counts(buffer, index_start..input_end);

    if options.extend_to_north_pole && min_y <= 0.0 {
        extrude_boundary_at_y(
            buffer,
            index_start..input_end,
            min_y,
            NORTH_POLE_Y,
            &edge_counts,
            cache,
        );
    }

    let south_extent = EXTENT_UINT as f32;
    if options.extend_to_south_pole && max_y >= south_extent {
        extrude_boundary_at_y(
            buffer,
            index_start..input_end,
            max_y,
            SOUTH_POLE_Y,
            &edge_counts,
            cache,
        );
    }
}

fn collect_edge_counts<I>(
    buffer: &VertexBuffers<ShaderVertex, I>,
    index_range: Range<usize>,
) -> (f32, f32, HashMap<(usize, usize), u32>)
where
    I: Copy + Into<u32>,
{
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    let mut edge_counts: HashMap<(usize, usize), u32> =
        HashMap::with_capacity_and_hasher(index_range.len(), Default::default());

    for chunk in buffer
        .indices
        .get(index_range)
        .unwrap_or(&[])
        .as_chunks::<3>()
        .0
    {
        let Some(triangle) = decode_triangle(chunk) else {
            continue;
        };
        for &vertex in &triangle {
            if let Some(v) = buffer.vertices.get(vertex) {
                min_y = min_y.min(v.position[1]);
                max_y = max_y.max(v.position[1]);
            }
        }
        for (a, b) in triangle_edges(triangle) {
            if a != b {
                let unordered = if a < b { (a, b) } else { (b, a) };
                *edge_counts.entry(unordered).or_insert(0) += 1;
            }
        }
    }
    (min_y, max_y, edge_counts)
}

fn decode_triangle<I: Copy + Into<u32>>(chunk: &[I]) -> Option<[usize; 3]> {
    let &[i0, i1, i2] = chunk else {
        return None;
    };
    Some([i0.into() as usize, i1.into() as usize, i2.into() as usize])
}

fn triangle_edges(triangle: [usize; 3]) -> [(usize, usize); 3] {
    [
        (triangle[0], triangle[1]),
        (triangle[1], triangle[2]),
        (triangle[2], triangle[0]),
    ]
}

fn extrude_boundary_at_y<I>(
    buffer: &mut VertexBuffers<ShaderVertex, I>,
    index_range: Range<usize>,
    target_y: f32,
    pole_y: f32,
    edge_counts: &HashMap<(usize, usize), u32>,
    cache: &mut HashMap<(i32, i32), usize>,
) where
    I: Copy + Into<u32> + From<VertexId>,
{
    let (boundary_edges, min_x, max_x, total_span) =
        collect_boundary_edges_at_y(buffer, index_range, target_y, edge_counts);

    // Only a polar-cap polygon spanning across the tile from its west meridian to its east
    // meridian encloses the pole; coastal bays or islands clipped within (0, EXTENT) — such as
    // the Ross Sea dipping past 85.05°S into the MVT south buffer — must not shoot wedges to 90°.
    let full_extent = EXTENT_UINT as f32;
    let spans_tile_width = min_x <= 0.0 && max_x >= full_extent && total_span >= full_extent * 0.5;
    if boundary_edges.is_empty() || !spans_tile_width {
        return;
    }

    for (first, second, first_position, second_position) in boundary_edges {
        append_pole_quad(
            buffer,
            first,
            second,
            first_position,
            second_position,
            pole_y,
            cache,
        );
    }
}

type BoundaryEdge = (usize, usize, [f32; 2], [f32; 2]);

fn collect_boundary_edges_at_y<I>(
    buffer: &VertexBuffers<ShaderVertex, I>,
    index_range: Range<usize>,
    target_y: f32,
    edge_counts: &HashMap<(usize, usize), u32>,
) -> (Vec<BoundaryEdge>, f32, f32, f32)
where
    I: Copy + Into<u32>,
{
    let mut boundary_edges = Vec::new();
    let mut total_span = 0.0_f32;
    let mut min_x = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    for chunk in buffer
        .indices
        .get(index_range)
        .unwrap_or(&[])
        .as_chunks::<3>()
        .0
    {
        let Some(triangle) = decode_triangle(chunk) else {
            continue;
        };
        for (first, second) in triangle_edges(triangle) {
            if first == second {
                continue;
            }
            let (Some(v_first), Some(v_second)) =
                (buffer.vertices.get(first), buffer.vertices.get(second))
            else {
                continue;
            };
            let first_pos = v_first.position;
            let second_pos = v_second.position;
            if first_pos[1] != target_y || second_pos[1] != target_y {
                continue;
            }
            let unordered = if first < second {
                (first, second)
            } else {
                (second, first)
            };
            if edge_counts.get(&unordered).copied() == Some(1) {
                min_x = min_x.min(first_pos[0].min(second_pos[0]));
                max_x = max_x.max(first_pos[0].max(second_pos[0]));
                total_span += (second_pos[0] - first_pos[0]).abs();
                boundary_edges.push((first, second, first_pos, second_pos));
            }
        }
    }
    (boundary_edges, min_x, max_x, total_span)
}

fn append_pole_quad<I>(
    buffer: &mut VertexBuffers<ShaderVertex, I>,
    first: usize,
    second: usize,
    first_position: [f32; 2],
    second_position: [f32; 2],
    pole_y: f32,
    cache: &mut HashMap<(i32, i32), usize>,
) where
    I: From<VertexId>,
{
    let first_pole = vertex_index(buffer, [first_position[0], pole_y], cache);
    let second_pole = vertex_index(buffer, [second_position[0], pole_y], cache);
    let flip = (first_position[0] > second_position[0]) != (pole_y == NORTH_POLE_Y);
    let triangles = if flip {
        [
            [first, second, first_pole],
            [second, second_pole, first_pole],
        ]
    } else {
        [
            [second, first, first_pole],
            [second_pole, second, first_pole],
        ]
    };
    for triangle in triangles {
        buffer
            .indices
            .extend(triangle.map(|index| I::from(VertexId::from_usize(index))));
    }
}
