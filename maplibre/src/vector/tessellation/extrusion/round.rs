//! Smooth circular corner filleting for `fill-extrusion-rounded-corner-distance`.

use std::f32::consts::PI;

use lyon::{
    math::point,
    path::Path,
    tessellation::{VertexBuffers, VertexId},
};

use super::{
    clip::{is_hole, normalized_rings, on_clipped_edge, rings, signed_area},
    ExtrusionHeights, MAX_EDGE_DISTANCE,
};
use crate::{render::ShaderVertex, style::layer::ElevationDatum};

pub(super) struct RoundedRing {
    pub points: Vec<[f32; 2]>,
    pub arc_normals: Vec<Option<[f32; 2]>>,
}

fn dedup_open_ring(ring: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let mut pts: Vec<[f32; 2]> = Vec::with_capacity(ring.len());
    for &p in ring {
        if pts
            .last()
            .is_none_or(|prev| (prev[0] - p[0]).hypot(prev[1] - p[1]) > 1e-4)
        {
            pts.push(p);
        }
    }
    if pts.len() > 1 {
        if let (Some(first), Some(last)) = (pts.first(), pts.last()) {
            if (first[0] - last[0]).hypot(first[1] - last[1]) <= 1e-4 {
                pts.pop();
            }
        }
    }
    pts
}

fn round_single_ring(
    open: &[[f32; 2]],
    distance: f32,
    facing: f32,
    suppress_tile_boundary: bool,
) -> RoundedRing {
    let n = open.len();
    if n < 3 || distance <= 0.0 {
        return RoundedRing {
            points: open.to_vec(),
            arc_normals: vec![None; n],
        };
    }

    let mut points = Vec::with_capacity(n * 4);
    let mut arc_normals = Vec::with_capacity(n * 4);

    for i in 0..n {
        let p_prev = open[(i + n - 1) % n];
        let p_curr = open[i];
        let p_next = open[(i + 1) % n];

        if on_clipped_edge(p_prev, p_curr, suppress_tile_boundary)
            || on_clipped_edge(p_curr, p_next, suppress_tile_boundary)
        {
            points.push(p_curr);
            arc_normals.push(None);
            continue;
        }

        let v_prev = [p_prev[0] - p_curr[0], p_prev[1] - p_curr[1]];
        let v_next = [p_next[0] - p_curr[0], p_next[1] - p_curr[1]];
        let len_prev = v_prev[0].hypot(v_prev[1]);
        let len_next = v_next[0].hypot(v_next[1]);
        let d = distance.min(0.5 * len_prev).min(0.5 * len_next);
        if len_prev <= 1e-4 || len_next <= 1e-4 || d <= 1e-4 {
            points.push(p_curr);
            arc_normals.push(None);
            continue;
        }

        let u_prev = [v_prev[0] / len_prev, v_prev[1] / len_prev];
        let u_next = [v_next[0] / len_next, v_next[1] / len_next];
        let dot = (u_prev[0] * u_next[0] + u_prev[1] * u_next[1]).clamp(-1.0, 1.0);
        let interior = dot.acos();
        if !(1e-2..(PI - 1e-2)).contains(&interior) {
            points.push(p_curr);
            arc_normals.push(None);
            continue;
        }

        let half = 0.5 * interior;
        let radius = d * half.tan();
        let bisector_raw = [u_prev[0] + u_next[0], u_prev[1] + u_next[1]];
        let bisector_len = bisector_raw[0].hypot(bisector_raw[1]);
        if radius <= 1e-4 || bisector_len <= 1e-5 {
            points.push(p_curr);
            arc_normals.push(None);
            continue;
        }
        let center_dist = d / half.cos();
        let center = [
            p_curr[0] + bisector_raw[0] / bisector_len * center_dist,
            p_curr[1] + bisector_raw[1] / bisector_len * center_dist,
        ];
        let t_start = [p_curr[0] + u_prev[0] * d, p_curr[1] + u_prev[1] * d];
        let t_end = [p_curr[0] + u_next[0] * d, p_curr[1] + u_next[1] * d];

        let a0 = (t_start[1] - center[1]).atan2(t_start[0] - center[0]);
        let a1 = (t_end[1] - center[1]).atan2(t_end[0] - center[0]);
        let mut sweep = a1 - a0;
        while sweep > PI {
            sweep -= 2.0 * PI;
        }
        while sweep < -PI {
            sweep += 2.0 * PI;
        }

        // Incoming edge runs `p_prev -> t_start` along `-u_prev`, so its outward normal is
        // `[-facing * u_prev[1], facing * u_prev[0]]`.
        let incoming_normal = [-facing * u_prev[1], facing * u_prev[0]];
        let radial_start = [
            (t_start[0] - center[0]) / radius,
            (t_start[1] - center[1]) / radius,
        ];
        let normal_sign =
            if radial_start[0] * incoming_normal[0] + radial_start[1] * incoming_normal[1] >= 0.0 {
                1.0_f32
            } else {
                -1.0_f32
            };

        let steps = ((sweep.abs() / (PI / 12.0)).ceil() as usize).clamp(4, 16);
        for k in 0..=steps {
            let frac = k as f32 / steps as f32;
            let angle = a0 + sweep * frac;
            let (sin, cos) = angle.sin_cos();
            let pos = if k == 0 {
                t_start
            } else if k == steps {
                t_end
            } else {
                [center[0] + radius * cos, center[1] + radius * sin]
            };
            let n = [normal_sign * cos, normal_sign * sin];
            if points
                .last()
                .is_some_and(|prev: &[f32; 2]| (prev[0] - pos[0]).hypot(prev[1] - pos[1]) <= 1e-4)
            {
                if let Some(last_norm) = arc_normals.last_mut() {
                    *last_norm = Some(n);
                }
                continue;
            }
            points.push(pos);
            arc_normals.push(Some(n));
        }
    }

    if points.len() > 1 {
        if let (Some(first), Some(last)) = (points.first().copied(), points.last().copied()) {
            if (first[0] - last[0]).hypot(first[1] - last[1]) <= 1e-4 {
                points.pop();
                arc_normals.pop();
            }
        }
    }

    RoundedRing {
        points,
        arc_normals,
    }
}

pub(super) fn build_rounded_extrusion(
    path: &Path,
    heights: ExtrusionHeights,
) -> Option<(Path, Vec<(RoundedRing, f32)>)> {
    if heights.rounded_corner_distance <= 0.0 {
        return None;
    }
    let raw_rings = rings(path);
    let active_rings = if heights.translucent {
        normalized_rings(&raw_rings)
    } else {
        raw_rings
    };
    let mut builder = Path::builder();
    let mut out_rings = Vec::with_capacity(active_rings.len());

    for (index, ring) in active_rings.iter().enumerate() {
        let area = signed_area(ring);
        if area == 0.0 {
            continue;
        }
        let facing = if is_hole(index, &active_rings) {
            area
        } else {
            -area
        }
        .signum();
        let open = dedup_open_ring(ring);
        if open.len() < 3 {
            continue;
        }
        let rounded = round_single_ring(
            &open,
            heights.rounded_corner_distance,
            facing,
            heights.suppress_tile_boundary_walls,
        );
        if rounded.points.len() < 3 {
            continue;
        }
        builder.begin(point(rounded.points[0][0], rounded.points[0][1]));
        for pt in &rounded.points[1..] {
            builder.line_to(point(pt[0], pt[1]));
        }
        builder.end(true);
        out_rings.push((rounded, facing));
    }

    Some((builder.build(), out_rings))
}

pub(super) fn push_smooth_wall<I: From<VertexId>>(
    buffer: &mut VertexBuffers<ShaderVertex, I>,
    [a, b]: [[f32; 2]; 2],
    [na, nb]: [[f32; 2]; 2],
    heights: ExtrusionHeights,
    edge_distances: [f32; 2],
) {
    let first = buffer.vertices.len() as u32;
    let index = |offset: u32| I::from(VertexId(first + offset));
    for (pt, norm, top, distance) in [
        (a, na, false, edge_distances[0]),
        (a, na, true, edge_distances[0]),
        (b, nb, true, edge_distances[1]),
        (b, nb, false, edge_distances[1]),
    ] {
        let scale = if top { 2.0 } else { 1.0 };
        let mut vertex = ShaderVertex::new(pt, [norm[0] * scale, norm[1] * scale]);
        heights.apply(&mut vertex, top);
        vertex.edge_distance = distance;
        buffer.vertices.push(vertex);
    }
    buffer.indices.extend([0, 1, 2, 0, 2, 3].map(index));
}

pub(super) fn emit_rounded_walls<I: From<VertexId>>(
    buffer: &mut VertexBuffers<ShaderVertex, I>,
    rings: &[(RoundedRing, f32)],
    heights: ExtrusionHeights,
) {
    let subdivide_agl_wall = heights.base_datum == ElevationDatum::VertexAgl
        || heights.height_datum == ElevationDatum::VertexAgl;
    for (rounded, facing) in rings {
        let n = rounded.points.len();
        if n < 3 {
            continue;
        }
        let mut travelled = 0.0_f32;
        for i in 0..n {
            let j = (i + 1) % n;
            let a = rounded.points[i];
            let b = rounded.points[j];
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let length = dx.hypot(dy);
            if on_clipped_edge(a, b, heights.suppress_tile_boundary_walls) || length <= 1e-5 {
                continue;
            }
            if travelled + length > MAX_EDGE_DISTANCE {
                travelled = 0.0;
            }
            let start = travelled;
            travelled += length;
            let flat_normal = [*facing * dy / length, -*facing * dx / length];
            let (na, nb) = match (rounded.arc_normals[i], rounded.arc_normals[j]) {
                (Some(na), Some(nb))
                    if na[0] * flat_normal[0] + na[1] * flat_normal[1] > 0.25
                        && nb[0] * flat_normal[0] + nb[1] * flat_normal[1] > 0.25 =>
                {
                    (na, nb)
                }
                _ => (flat_normal, flat_normal),
            };
            let steps = if subdivide_agl_wall {
                (length / 128.0).ceil().max(1.0) as u32
            } else {
                1
            };
            for step in 0..steps {
                let t0 = step as f32 / steps as f32;
                let t1 = (step + 1) as f32 / steps as f32;
                let p0 = [a[0] + dx * t0, a[1] + dy * t0];
                let p1 = if step + 1 == steps {
                    b
                } else {
                    [a[0] + dx * t1, a[1] + dy * t1]
                };
                let lerp_norm = |t: f32| {
                    let rx = na[0] + (nb[0] - na[0]) * t;
                    let ry = na[1] + (nb[1] - na[1]) * t;
                    let len = rx.hypot(ry).max(1e-6);
                    [rx / len, ry / len]
                };
                let d0 = start + length * (1.0 - t0);
                let d1 = if step + 1 == steps {
                    start
                } else {
                    start + length * (1.0 - t1)
                };
                push_smooth_wall(
                    buffer,
                    [p0, p1],
                    [lerp_norm(t0), lerp_norm(t1)],
                    heights,
                    [d0, d1],
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use lyon::{math::point, path::Path, tessellation::VertexBuffers};

    use crate::{
        render::ShaderVertex,
        style::layer::StyleLayer,
        vector::tessellation::extrusion::{extrude, ExtrusionHeights},
    };

    fn square_path([x0, y0, x1, y1]: [f32; 4]) -> Path {
        let mut builder = Path::builder();
        builder.begin(point(x0, y0));
        builder.line_to(point(x1, y0));
        builder.line_to(point(x1, y1));
        builder.line_to(point(x0, y1));
        builder.end(true);
        builder.build()
    }

    #[test]
    fn rounded_corner_distance_generates_smooth_fillet_walls_and_clamps_to_half_segment() {
        let sharp_heights = ExtrusionHeights::constant(0.0, 20.0);
        let mut sharp_buf = VertexBuffers::<ShaderVertex, u32>::new();
        extrude(
            &square_path([100.0, 100.0, 300.0, 300.0]),
            sharp_heights,
            &mut sharp_buf,
            (0.02, |_, _| Ok(())),
        )
        .expect("sharp extrudes");

        let mut rounded_heights = ExtrusionHeights::constant(0.0, 20.0);
        rounded_heights.rounded_corner_distance = 40.0;
        let mut rounded_buf = VertexBuffers::<ShaderVertex, u32>::new();
        extrude(
            &square_path([100.0, 100.0, 300.0, 300.0]),
            rounded_heights,
            &mut rounded_buf,
            (0.02, |_, _| Ok(())),
        )
        .expect("rounded extrudes");

        let sharp_walls = sharp_buf
            .vertices
            .iter()
            .filter(|v| v.normal != [0.0, 0.0])
            .count();
        let rounded_walls: Vec<&ShaderVertex> = rounded_buf
            .vertices
            .iter()
            .filter(|v| v.normal != [0.0, 0.0])
            .collect();
        assert!(
            rounded_walls.len() > sharp_walls,
            "rounded square must emit smooth arc wall segments ({} > {sharp_walls})",
            rounded_walls.len()
        );
        // None of the original sharp corners `(100, 100)`, `(300, 100)` remain on the rounded hull.
        assert!(!rounded_buf
            .vertices
            .iter()
            .any(|v| (v.position[0] - 100.0).abs() < 1e-3 && (v.position[1] - 100.0).abs() < 1e-3));
        // Diagonal normals exist on the rounded corner arcs.
        assert!(rounded_walls.iter().any(|v| {
            let len = v.normal[0].hypot(v.normal[1]);
            let nx = (v.normal[0] / len).abs();
            let ny = (v.normal[1] / len).abs();
            nx > 0.3 && ny > 0.3
        }));

        // Clamping test: radius larger than half the 200-unit edge still stays inside [100..300].
        let mut huge_heights = ExtrusionHeights::constant(0.0, 20.0);
        huge_heights.rounded_corner_distance = 500.0;
        let mut huge_buf = VertexBuffers::<ShaderVertex, u32>::new();
        extrude(
            &square_path([100.0, 100.0, 300.0, 300.0]),
            huge_heights,
            &mut huge_buf,
            (0.02, |_, _| Ok(())),
        )
        .expect("clamped extrudes");
        for v in &huge_buf.vertices {
            assert!((99.9..=300.1).contains(&v.position[0]));
            assert!((99.9..=300.1).contains(&v.position[1]));
        }
    }

    #[test]
    fn layer_accepts_rounded_corner_distance_in_paint_or_layout() {
        let from_paint: StyleLayer = serde_json::from_value(serde_json::json!({
            "id": "b1",
            "type": "fill-extrusion",
            "source": "s",
            "paint": { "fill-extrusion-rounded-corner-distance": 8.0 }
        }))
        .expect("paint parses");
        let from_layout: StyleLayer = serde_json::from_value(serde_json::json!({
            "id": "b2",
            "type": "fill-extrusion",
            "source": "s",
            "layout": { "fill-extrusion-rounded-corner-distance": 8.0 }
        }))
        .expect("layout parses");
        let style = crate::style::Style::default();
        assert!(style.validate_layer(&from_paint).is_empty());
        assert!(style.validate_layer(&from_layout).is_empty());
    }
}
