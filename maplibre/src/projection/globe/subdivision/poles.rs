//! Extrudes clipped polar tile boundaries of subdivided fill polygons to the globe poles.

use std::{collections::HashMap, ops::Range};

use lyon::tessellation::{VertexBuffers, VertexId};

use super::{vertex_index, FillSubdivisionOptions, NORTH_POLE_Y, SOUTH_POLE_Y};
use crate::{coords::EXTENT_UINT, render::ShaderVertex};

/// Minimum horizontal span (in 4096-extent tile units) required to treat an unpaired horizontal
/// edge inside the tile buffer (`y < 0` or `y > EXTENT`) as a tile-clipped polar boundary rather
/// than a quantized natural tangent at the tip of a closed coastal bay or island.
const MIN_BUFFERED_POLE_CLIP_SPAN: f32 = 8.0;

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

    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    let mut edge_counts: HashMap<(usize, usize), u32> =
        HashMap::with_capacity(input_end - index_start);

    for offset in (index_start..input_end).step_by(3) {
        let triangle = [
            buffer.indices[offset].into() as usize,
            buffer.indices[offset + 1].into() as usize,
            buffer.indices[offset + 2].into() as usize,
        ];
        for &vertex in &triangle {
            let y = buffer.vertices[vertex].position[1];
            min_y = min_y.min(y);
            max_y = max_y.max(y);
        }
        for edge in 0..3 {
            let a = triangle[edge];
            let b = triangle[(edge + 1) % 3];
            if a != b {
                let unordered = if a < b { (a, b) } else { (b, a) };
                *edge_counts.entry(unordered).or_insert(0) += 1;
            }
        }
    }

    if options.extend_to_north_pole && min_y <= 0.0 {
        extrude_boundary_at_y(
            buffer,
            index_start..input_end,
            min_y,
            min_y < 0.0,
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
            max_y > south_extent,
            SOUTH_POLE_Y,
            &edge_counts,
            cache,
        );
    }
}

fn extrude_boundary_at_y<I>(
    buffer: &mut VertexBuffers<ShaderVertex, I>,
    index_range: Range<usize>,
    target_y: f32,
    in_buffer: bool,
    pole_y: f32,
    edge_counts: &HashMap<(usize, usize), u32>,
    cache: &mut HashMap<(i32, i32), usize>,
) where
    I: Copy + Into<u32> + From<VertexId>,
{
    let mut boundary_edges = Vec::new();
    let mut total_span = 0.0_f32;
    for offset in index_range.step_by(3) {
        let triangle = [
            buffer.indices[offset].into() as usize,
            buffer.indices[offset + 1].into() as usize,
            buffer.indices[offset + 2].into() as usize,
        ];
        for edge in 0..3 {
            let first = triangle[edge];
            let second = triangle[(edge + 1) % 3];
            if first == second {
                continue;
            }
            let first_pos = buffer.vertices[first].position;
            let second_pos = buffer.vertices[second].position;
            if first_pos[1] != target_y || second_pos[1] != target_y {
                continue;
            }
            let unordered = if first < second {
                (first, second)
            } else {
                (second, first)
            };
            if edge_counts.get(&unordered).copied() == Some(1) {
                total_span += (second_pos[0] - first_pos[0]).abs();
                boundary_edges.push((first, second, first_pos, second_pos));
            }
        }
    }

    if boundary_edges.is_empty() || (in_buffer && total_span < MIN_BUFFERED_POLE_CLIP_SPAN) {
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
