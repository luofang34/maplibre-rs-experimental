//! Fast-path `earcut` polygon triangulation for simple exterior + hole rings, falling back to
//! `lyon` when curves, self-intersections, or non-standard multi-ring winding appear.

use earcut::Earcut;
use lyon::{
    path::{Event, Path},
    tessellation::{VertexBuffers, VertexId},
};

use super::MaxIndex;
use crate::render::ShaderVertex;

/// Maximum vertex count per polygon handled on the verified `earcut` fast path; larger coastlines
/// use `lyon`'s sweep-line tessellator.
const MAX_SIMPLE_VERTICES: usize = 192;

/// Reusable scratch buffers for zero-allocation per-polygon `earcut` triangulation.
#[derive(Default)]
pub(crate) struct PolygonEarcut {
    earcut: Earcut<f32>,
    rings: Vec<Vec<[f32; 2]>>,
    flat_vertices: Vec<[f32; 2]>,
    hole_indices: Vec<u32>,
    triangles: Vec<u32>,
}

impl PolygonEarcut {
    /// Triangulates `path` directly into `buffer` when `path` is a clean simple polygon (with
    /// optional strictly interior non-crossing holes) whose fill matches `even_odd` (`false` =
    /// `FillRule::NonZero`, `true` = `FillRule::EvenOdd`). Returns `false` without modifying
    /// `buffer` when `lyon` should handle `path`.
    pub(crate) fn try_triangulate<I, F>(
        &mut self,
        path: &Path,
        even_odd: bool,
        buffer: &mut VertexBuffers<ShaderVertex, I>,
        mut make_vertex: F,
    ) -> bool
    where
        I: From<VertexId> + MaxIndex + Copy + Into<u32>,
        F: FnMut([f32; 2]) -> ShaderVertex,
    {
        if !self.extract_rings(path) {
            return false;
        }
        if self.rings.is_empty() {
            return true;
        }
        let total_vertices: usize = self.rings.iter().map(Vec::len).sum();
        if total_vertices > MAX_SIMPLE_VERTICES {
            return false;
        }
        let Some(last_vertex) = buffer
            .vertices
            .len()
            .checked_add(total_vertices)
            .and_then(|end| end.checked_sub(1))
        else {
            return false;
        };
        if last_vertex > I::MAX {
            return false;
        }

        if !validate_ring_topology(&self.rings, even_odd) || rings_have_intersections(&self.rings) {
            return false;
        }

        self.flat_vertices.clear();
        self.hole_indices.clear();
        for (idx, ring) in self.rings.iter().enumerate() {
            if idx > 0 {
                self.hole_indices.push(self.flat_vertices.len() as u32);
            }
            self.flat_vertices.extend_from_slice(ring);
        }

        self.earcut.earcut(
            self.flat_vertices.iter().copied(),
            &self.hole_indices,
            &mut self.triangles,
        );

        let expected_triangles = total_vertices + 2 * self.hole_indices.len() - 2;
        if self.triangles.len() != expected_triangles * 3 {
            return false;
        }

        let base = buffer.vertices.len() as u32;
        buffer.vertices.reserve(self.flat_vertices.len());
        for &pt in &self.flat_vertices {
            buffer.vertices.push(make_vertex(pt));
        }
        buffer.indices.reserve(self.triangles.len());
        for &idx in &self.triangles {
            buffer.indices.push(I::from(VertexId(base + idx)));
        }
        true
    }

    fn extract_rings(&mut self, path: &Path) -> bool {
        self.rings.clear();
        let mut current: Vec<[f32; 2]> = Vec::new();
        for event in path.iter() {
            match event {
                Event::Begin { at } => {
                    current.clear();
                    current.push(at.to_array());
                }
                Event::Line { to, .. } => {
                    let pt = to.to_array();
                    if current.last().copied() != Some(pt) {
                        current.push(pt);
                    }
                }
                Event::End { .. } => {
                    if current.len() >= 2 && current.first() == current.last() {
                        current.pop();
                    }
                    if current.is_empty() {
                        continue;
                    }
                    if current.len() < 3 {
                        return false;
                    }
                    self.rings.push(std::mem::take(&mut current));
                }
                Event::Quadratic { .. } | Event::Cubic { .. } => return false,
            }
        }
        true
    }
}

fn signed_area(ring: &[[f32; 2]]) -> f32 {
    ring.iter()
        .zip(ring.iter().cycle().skip(1))
        .map(|(a, b)| a[0] * b[1] - b[0] * a[1])
        .sum()
}

fn point_in_ring(ring: &[[f32; 2]], [x, y]: [f32; 2]) -> bool {
    let mut inside = false;
    for (a, b) in ring.iter().zip(ring.iter().cycle().skip(1)) {
        if (a[1] > y) != (b[1] > y) && x < (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
    }
    inside
}

fn validate_ring_topology(rings: &[Vec<[f32; 2]>], even_odd: bool) -> bool {
    let outer_area = signed_area(&rings[0]);
    if !outer_area.is_finite() || outer_area.abs() < 1e-4 {
        return false;
    }
    for (i, hole) in rings.iter().enumerate().skip(1) {
        let area = signed_area(hole);
        if !area.is_finite() || area.abs() < 1e-4 {
            return false;
        }
        if !even_odd && outer_area * area > 0.0 {
            return false;
        }
        if !point_in_ring(&rings[0], hole[0]) {
            return false;
        }
        for (j, other_hole) in rings.iter().enumerate().skip(1) {
            if i != j && point_in_ring(other_hole, hole[0]) {
                return false;
            }
        }
    }
    true
}

fn rings_have_intersections(rings: &[Vec<[f32; 2]>]) -> bool {
    for (r0, ring_a) in rings.iter().enumerate() {
        let len_a = ring_a.len();
        for e0 in 0..len_a {
            let a0 = ring_a[e0];
            let a1 = ring_a[(e0 + 1) % len_a];
            let ax_min = a0[0].min(a1[0]);
            let ax_max = a0[0].max(a1[0]);
            let ay_min = a0[1].min(a1[1]);
            let ay_max = a0[1].max(a1[1]);

            for (r1, ring_b) in rings.iter().enumerate().skip(r0) {
                let len_b = ring_b.len();
                let start_e1 = if r0 == r1 { e0 + 1 } else { 0 };
                for e1 in start_e1..len_b {
                    if r0 == r1 && (e1 == e0 + 1 || (e0 == 0 && e1 + 1 == len_b)) {
                        continue;
                    }
                    let b0 = ring_b[e1];
                    let b1 = ring_b[(e1 + 1) % len_b];
                    if ax_max < b0[0].min(b1[0])
                        || b0[0].max(b1[0]) < ax_min
                        || ay_max < b0[1].min(b1[1])
                        || b0[1].max(b1[1]) < ay_min
                    {
                        continue;
                    }
                    if segments_touch_or_cross(a0, a1, b0, b1) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn cross(o: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

fn on_segment(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> bool {
    p[0] >= a[0].min(b[0]) - 1e-5
        && p[0] <= a[0].max(b[0]) + 1e-5
        && p[1] >= a[1].min(b[1]) - 1e-5
        && p[1] <= a[1].max(b[1]) + 1e-5
}

fn segments_touch_or_cross(a0: [f32; 2], a1: [f32; 2], b0: [f32; 2], b1: [f32; 2]) -> bool {
    let d1 = cross(b0, b1, a0);
    let d2 = cross(b0, b1, a1);
    let d3 = cross(a0, a1, b0);
    let d4 = cross(a0, a1, b1);

    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    {
        return true;
    }
    (d1.abs() <= 1e-5 && on_segment(a0, b0, b1))
        || (d2.abs() <= 1e-5 && on_segment(a1, b0, b1))
        || (d3.abs() <= 1e-5 && on_segment(b0, a0, a1))
        || (d4.abs() <= 1e-5 && on_segment(b1, a0, a1))
}

#[cfg(test)]
mod tests {
    use lyon::{
        math::Point,
        path::Path,
        tessellation::{BuffersBuilder, FillOptions, FillRule, FillTessellator, VertexBuffers},
    };

    use super::PolygonEarcut;
    use crate::{render::ShaderVertex, vector::tessellation::VertexConstructor};

    fn build_path(rings: &[&[[f32; 2]]]) -> Path {
        let mut builder = Path::builder();
        for ring in rings {
            builder.begin(Point::new(ring[0][0], ring[0][1]));
            for pt in &ring[1..] {
                builder.line_to(Point::new(pt[0], pt[1]));
            }
            builder.end(true);
        }
        builder.build()
    }

    fn total_unsigned_area(buf: &VertexBuffers<ShaderVertex, u32>) -> f32 {
        buf.indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|tri| {
                let a = buf.vertices[tri[0] as usize].position;
                let b = buf.vertices[tri[1] as usize].position;
                let c = buf.vertices[tri[2] as usize].position;
                0.5 * ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs()
            })
            .sum()
    }

    #[test]
    fn concave_and_holed_polygons_match_lyon_area() {
        let outer: &[[f32; 2]] = &[
            [0.0, 0.0],
            [100.0, 0.0],
            [100.0, 40.0],
            [40.0, 40.0],
            [40.0, 100.0],
            [0.0, 100.0],
        ];
        let hole: &[[f32; 2]] = &[[10.0, 10.0], [10.0, 30.0], [30.0, 30.0], [30.0, 10.0]];
        let path = build_path(&[outer, hole]);

        let mut earcut = PolygonEarcut::default();
        let mut fast_buf = VertexBuffers::<ShaderVertex, u32>::new();
        assert!(earcut.try_triangulate(&path, false, &mut fast_buf, |pos| {
            ShaderVertex::new(pos, [0.0, 0.0])
        }));

        let mut lyon_buf = VertexBuffers::<ShaderVertex, u32>::new();
        FillTessellator::new()
            .tessellate_path(
                &path,
                &FillOptions::default().with_fill_rule(FillRule::NonZero),
                &mut BuffersBuilder::new(&mut lyon_buf, VertexConstructor {}),
            )
            .expect("lyon fill");

        assert!((total_unsigned_area(&fast_buf) - total_unsigned_area(&lyon_buf)).abs() < 1e-2);
    }

    #[test]
    fn self_intersecting_and_same_wound_nonzero_rings_fall_back_to_lyon() {
        let mut earcut = PolygonEarcut::default();
        let mut buf = VertexBuffers::<ShaderVertex, u32>::new();

        // Bowtie self-intersection must fall back.
        let bowtie = build_path(&[&[[0.0, 0.0], [100.0, 100.0], [100.0, 0.0], [0.0, 100.0]]]);
        assert!(!earcut.try_triangulate(&bowtie, false, &mut buf, |pos| {
            ShaderVertex::new(pos, [0.0, 0.0])
        }));
        assert!(buf.vertices.is_empty());

        // Same-wound concentric rings reject NonZero (`even_odd = false`) but accept EvenOdd (`true`).
        let outer: &[[f32; 2]] = &[[0.0, 0.0], [100.0, 0.0], [100.0, 100.0], [0.0, 100.0]];
        let inner_same_winding: &[[f32; 2]] =
            &[[20.0, 20.0], [80.0, 20.0], [80.0, 80.0], [20.0, 80.0]];
        let concentric = build_path(&[outer, inner_same_winding]);
        assert!(
            !earcut.try_triangulate(&concentric, false, &mut buf, |pos| {
                ShaderVertex::new(pos, [0.0, 0.0])
            })
        );
        assert!(earcut.try_triangulate(&concentric, true, &mut buf, |pos| {
            ShaderVertex::new(pos, [0.0, 0.0])
        }));
        assert!((total_unsigned_area(&buf) - 6400.0).abs() < 1e-2);
    }
}
