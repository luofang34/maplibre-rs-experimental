//! Splits unbounded concave miter vertices emitted by `lyon_tessellation`.
//!
//! `lyon_tessellation` evaluates segment-folding heuristics against `half_width` in tile units
//! (`0.5` by default), whereas line and fill-outline shaders extrude vertex normals in screen
//! pixels (`line-width` or `1 px`) independent of tile-to-screen zoom. On sharp `< 1°` coastal
//! zig-zags whose edges exceed `0.5` tile units yet remain shorter than the screen-space miter
//! reach, `lyon` leaves the concave (`back_side`) corner as an un-folded single miter vertex with
//! `|normal| = 1 / sin(theta / 2)`, producing long spikes without pinching only if split into two
//! segment-perpendicular vertices (`inner_join`).

use lyon::tessellation::{VertexBuffers, VertexId};

use crate::render::ShaderVertex;

#[derive(Clone, Copy)]
struct SplitJoin {
    spiky_vertex: usize,
    join_tri_offset: usize,
    front_prev: usize,
    front_next: usize,
    back_is_positive: bool,
}

/// Replaces any concave join `single_vertex` in `vertex_start..` whose normal magnitude exceeds
/// `max_normal` with an `inner_join` bevel pair preserving full perpendicular stroke width on both
/// incident segments.
pub(super) fn split_unclipped_concave_joins<I>(
    buffer: &mut VertexBuffers<ShaderVertex, I>,
    vertex_start: usize,
    index_start: usize,
    max_normal: f32,
) where
    I: Copy + Into<u32> + From<VertexId>,
{
    let vertex_end = buffer.vertices.len();
    let index_end = buffer.indices.len();
    if vertex_start >= vertex_end || index_start >= index_end {
        return;
    }
    let max_sq = max_normal * max_normal;
    let norm_sq = |n: [f32; 2]| n[0] * n[0] + n[1] * n[1];
    if !buffer.vertices[vertex_start..vertex_end]
        .iter()
        .any(|v| norm_sq(v.normal) > max_sq)
    {
        return;
    }

    let mut is_spiky: Vec<bool> = buffer.vertices[vertex_start..vertex_end]
        .iter()
        .map(|v| norm_sq(v.normal) > max_sq)
        .collect();
    let spiky = |idx: usize, flags: &[bool]| -> bool {
        idx.checked_sub(vertex_start)
            .and_then(|s| flags.get(s).copied())
            .unwrap_or(false)
    };

    let mut splits: Vec<Option<SplitJoin>> = vec![None; vertex_end - vertex_start];
    for tri_offset in (index_start..index_end).step_by(3) {
        let i0 = buffer.indices[tri_offset].into() as usize;
        let i1 = buffer.indices[tri_offset + 1].into() as usize;
        let i2 = buffer.indices[tri_offset + 2].into() as usize;
        let p0 = buffer.vertices[i0].position;
        if p0 != buffer.vertices[i1].position || p0 != buffer.vertices[i2].position {
            continue;
        }
        let (s0, s1, s2) = (
            spiky(i0, &is_spiky),
            spiky(i1, &is_spiky),
            spiky(i2, &is_spiky),
        );
        if u8::from(s0) + u8::from(s1) + u8::from(s2) != 1 {
            continue;
        }
        let (v, b, c) = if s0 {
            (i0, i1, i2)
        } else if s1 {
            (i1, i2, i0)
        } else {
            (i2, i0, i1)
        };
        if b == c {
            continue;
        }
        let (front_prev, front_next, back_is_positive) =
            if b < c { (b, c, false) } else { (c, b, true) };
        splits[v - vertex_start] = Some(SplitJoin {
            spiky_vertex: v,
            join_tri_offset: tri_offset,
            front_prev,
            front_next,
            back_is_positive,
        });
    }

    let mut back_next_for = vec![0_usize; vertex_end - vertex_start];
    for (slot, maybe_split) in splits.iter().enumerate() {
        let Some(split) = maybe_split else {
            continue;
        };
        let prev_front = buffer.vertices[split.front_prev];
        let next_front = buffer.vertices[split.front_next];
        let mut back_next_vertex = buffer.vertices[split.spiky_vertex];
        buffer.vertices[split.spiky_vertex].normal = [-prev_front.normal[0], -prev_front.normal[1]];
        buffer.vertices[split.spiky_vertex].distance = prev_front.distance;
        back_next_vertex.normal = [-next_front.normal[0], -next_front.normal[1]];
        back_next_vertex.distance = next_front.distance;
        let back_next_idx = buffer.vertices.len();
        buffer.vertices.push(back_next_vertex);
        back_next_for[slot] = back_next_idx;
        is_spiky[slot] = false;
    }

    for tri_offset in (index_start..index_end).step_by(3) {
        let tri = [
            buffer.indices[tri_offset].into() as usize,
            buffer.indices[tri_offset + 1].into() as usize,
            buffer.indices[tri_offset + 2].into() as usize,
        ];
        let join_match = tri.iter().find_map(|&idx| {
            idx.checked_sub(vertex_start)
                .and_then(|s| splits.get(s).copied().flatten())
                .filter(|split| split.join_tri_offset == tri_offset)
        });
        if let Some(split) = join_match {
            let fp = I::from(VertexId::from_usize(split.front_prev));
            let fnx = I::from(VertexId::from_usize(split.front_next));
            let bp = I::from(VertexId::from_usize(split.spiky_vertex));
            let bn = I::from(VertexId::from_usize(
                back_next_for[split.spiky_vertex - vertex_start],
            ));
            if split.back_is_positive {
                buffer.indices[tri_offset] = fp;
                buffer.indices[tri_offset + 1] = bp;
                buffer.indices[tri_offset + 2] = fnx;
                buffer.indices.extend([fp, bn, bp]);
            } else {
                buffer.indices[tri_offset] = fp;
                buffer.indices[tri_offset + 1] = fnx;
                buffer.indices[tri_offset + 2] = bp;
                buffer.indices.extend([fp, bp, bn]);
            }
            continue;
        }

        for (corner, &idx) in tri.iter().enumerate() {
            let Some(slot) = idx.checked_sub(vertex_start) else {
                continue;
            };
            let Some(split) = splits.get(slot).copied().flatten() else {
                continue;
            };
            let use_back_next = if split.back_is_positive {
                tri.contains(&split.front_next)
            } else {
                !tri.contains(&split.front_prev)
            };
            if use_back_next {
                buffer.indices[tri_offset + corner] =
                    I::from(VertexId::from_usize(back_next_for[slot]));
            }
        }
    }

    if is_spiky.iter().any(|&flag| flag) {
        recover_closed_start_normals(
            buffer,
            vertex_start,
            index_start,
            index_end,
            max_normal,
            &mut is_spiky,
        );
    }
}

fn recover_closed_start_normals<I>(
    buffer: &mut VertexBuffers<ShaderVertex, I>,
    vertex_start: usize,
    index_start: usize,
    index_end: usize,
    max_normal: f32,
    is_spiky: &mut [bool],
) where
    I: Copy + Into<u32>,
{
    let spiky = |idx: usize, flags: &[bool]| -> bool {
        idx.checked_sub(vertex_start)
            .and_then(|s| flags.get(s).copied())
            .unwrap_or(false)
    };
    for tri_offset in (index_start..index_end).step_by(3) {
        let tri = [
            buffer.indices[tri_offset].into() as usize,
            buffer.indices[tri_offset + 1].into() as usize,
            buffer.indices[tri_offset + 2].into() as usize,
        ];
        for &idx in &tri {
            if !spiky(idx, is_spiky) {
                continue;
            }
            let pos = buffer.vertices[idx].position;
            if let Some(&partner) = tri.iter().find(|&&other| {
                other != idx && !spiky(other, is_spiky) && buffer.vertices[other].position == pos
            }) {
                let pn = buffer.vertices[partner].normal;
                buffer.vertices[idx].normal = [-pn[0], -pn[1]];
                is_spiky[idx - vertex_start] = false;
            }
        }
    }
    for (offset, &still_spiky) in is_spiky.iter().enumerate() {
        if still_spiky {
            let v = &mut buffer.vertices[vertex_start + offset];
            let len = (v.normal[0] * v.normal[0] + v.normal[1] * v.normal[1]).sqrt();
            if len > max_normal {
                let scale = max_normal / len;
                v.normal[0] *= scale;
                v.normal[1] *= scale;
            }
        }
    }
}

#[cfg(test)]
mod tests;
