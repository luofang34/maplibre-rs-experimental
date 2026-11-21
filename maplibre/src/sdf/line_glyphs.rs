//! Glyph positions along a polyline, for labels whose text follows the line.
//!
//! Glyphs sit at fixed distances from the label's anchor along the line, so a bend turns
//! them with it instead of leaving them on one straight baseline. Distances and points are
//! tile units, the space of the label's map-aligned plane.

/// Where one glyph's centre lies and which way it points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GlyphPose {
    /// Centre of the glyph in tile units.
    pub point: [f32; 2],
    /// Direction of the line under the glyph in radians, turned half way around when flipped.
    pub angle: f32,
    /// How far the line under the glyph stands above the ground under the label's anchor, in
    /// metres, for a label that follows the ground: the ground under the ends of the glyph's
    /// segment of the line, interpolated along it. Zero otherwise.
    pub rise: f32,
}

/// Finds points by arc length along a polyline, resuming where the last one was found so that
/// a run of ascending distances walks the line once.
struct LineWalker<'a> {
    polyline: &'a [[f32; 2]],
    segment: usize,
    travelled: f32,
}

impl<'a> LineWalker<'a> {
    fn new(polyline: &'a [[f32; 2]]) -> Self {
        Self {
            polyline,
            segment: 0,
            travelled: 0.0,
        }
    }

    /// The point at arc length `distance` from the start of the polyline and the direction of
    /// the segment it lies on; `None` when the line is too short to reach it. Distances must
    /// not decrease between calls.
    fn point_at(&mut self, distance: f32) -> Option<([f32; 2], f32)> {
        if distance < 0.0 {
            return None;
        }
        while let [from, to] = self.polyline.get(self.segment..self.segment + 2)? {
            let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
            let length = dx.hypot(dy);
            if length > 0.0 && distance <= self.travelled + length {
                let t = (distance - self.travelled) / length;
                return Some(([from[0] + dx * t, from[1] + dy * t], dy.atan2(dx)));
            }
            self.travelled += length;
            self.segment += 1;
        }
        None
    }
}

/// Offsets `polyline` perpendicularly by `offset` (in tile units, positive to the right of travel
/// in Y-down tile coordinates), intersecting adjacent offset segments at corners and projecting
/// the anchor at `anchor_distance` onto the offset polyline.
pub(crate) fn offset_polyline(
    polyline: &[[f32; 2]],
    anchor_distance: f32,
    offset: f32,
) -> (Vec<[f32; 2]>, f32) {
    if offset.abs() <= 1e-4 || polyline.len() < 2 {
        return (polyline.to_vec(), anchor_distance);
    }

    struct OffsetSegment {
        start: [f32; 2],
        end: [f32; 2],
        tangent: [f32; 2],
        normal: [f32; 2],
    }

    // Collect non-degenerate segment tangents and unit normals.
    let mut segs: Vec<OffsetSegment> = Vec::with_capacity(polyline.len());
    for pair in polyline.windows(2) {
        let dx = pair[1][0] - pair[0][0];
        let dy = pair[1][1] - pair[0][1];
        let len = dx.hypot(dy);
        if len > 1e-6 {
            let tangent = [dx / len, dy / len];
            let normal = [-tangent[1], tangent[0]];
            segs.push(OffsetSegment {
                start: pair[0],
                end: pair[1],
                tangent,
                normal,
            });
        }
    }
    if segs.is_empty() {
        return (polyline.to_vec(), anchor_distance);
    }

    // Original anchor point in tile coordinates, so we can project it onto the offset polyline.
    let mut anchor_pt = segs[0].start;
    {
        let mut walker = LineWalker::new(polyline);
        if let Some((pt, _)) = walker.point_at(anchor_distance.max(0.0)) {
            anchor_pt = pt;
        }
    }

    let miter_limit = offset.abs() * 6.0;
    let mut out: Vec<[f32; 2]> = Vec::with_capacity(segs.len() + 2);
    out.push([
        segs[0].start[0] + segs[0].normal[0] * offset,
        segs[0].start[1] + segs[0].normal[1] * offset,
    ]);

    for pair in segs.windows(2) {
        let (vertex, t0, n0) = (pair[0].end, pair[0].tangent, pair[0].normal);
        let (t1, n1) = (pair[1].tangent, pair[1].normal);
        let det = t0[0] * t1[1] - t0[1] * t1[0];
        if det.abs() < 1e-5 {
            out.push([vertex[0] + n1[0] * offset, vertex[1] + n1[1] * offset]);
            continue;
        }
        let dp = [(n1[0] - n0[0]) * offset, (n1[1] - n0[1]) * offset];
        let s = (dp[0] * t1[1] - dp[1] * t1[0]) / det;
        let miter = [
            vertex[0] + n0[0] * offset + t0[0] * s,
            vertex[1] + n0[1] * offset + t0[1] * s,
        ];
        if (miter[0] - vertex[0]).hypot(miter[1] - vertex[1]) <= miter_limit {
            out.push(miter);
        } else {
            out.push([vertex[0] + n0[0] * offset, vertex[1] + n0[1] * offset]);
            out.push([vertex[0] + n1[0] * offset, vertex[1] + n1[1] * offset]);
        }
    }

    if let Some(last) = segs.last() {
        out.push([
            last.end[0] + last.normal[0] * offset,
            last.end[1] + last.normal[1] * offset,
        ]);
    }

    // Project `anchor_pt` onto the offset polyline to obtain the matching anchor arc length.
    let mut best_dist_sq = f32::INFINITY;
    let mut best_arc = 0.0_f32;
    let mut travelled = 0.0_f32;
    for pair in out.windows(2) {
        let dx = pair[1][0] - pair[0][0];
        let dy = pair[1][1] - pair[0][1];
        let len_sq = dx * dx + dy * dy;
        let len = len_sq.sqrt();
        let t = if len_sq > 1e-12 {
            (((anchor_pt[0] - pair[0][0]) * dx + (anchor_pt[1] - pair[0][1]) * dy) / len_sq)
                .clamp(0.0, 1.0)
        } else {
            0.0
        };
        let px = pair[0][0] + dx * t;
        let py = pair[0][1] + dy * t;
        let d_sq = (px - anchor_pt[0]).powi(2) + (py - anchor_pt[1]).powi(2);
        if d_sq < best_dist_sq {
            best_dist_sq = d_sq;
            best_arc = travelled + len * t;
        }
        travelled += len;
    }

    (out, best_arc)
}

/// Poses of glyphs whose centres are `offsets` (tile units, signed) from the anchor at
/// `anchor_distance` along `polyline`. `flip` reads the text against the line direction: each
/// glyph takes the mirrored distance and turns half way around. `None` when any glyph would
/// lie beyond either end of the line, in which case the label is not drawn.
pub(crate) fn place_glyphs(
    polyline: &[[f32; 2]],
    anchor_distance: f32,
    offsets: &[f32],
    flip: bool,
) -> Option<Vec<GlyphPose>> {
    let distances: Vec<f32> = offsets
        .iter()
        .map(|offset| {
            if flip {
                anchor_distance - offset
            } else {
                anchor_distance + offset
            }
        })
        .collect();
    // Glyphs are found in order of distance with one walk along the line.
    let mut order: Vec<usize> = (0..distances.len()).collect();
    order.sort_by(|a, b| distances[*a].total_cmp(&distances[*b]));
    let mut walker = LineWalker::new(polyline);
    let mut poses = vec![None; distances.len()];
    for index in order {
        let (point, angle) = walker.point_at(distances[index])?;
        poses[index] = Some(GlyphPose {
            point,
            angle: if flip {
                angle + std::f32::consts::PI
            } else {
                angle
            },
            rise: 0.0,
        });
    }
    poses.into_iter().collect()
}

/// Whether text that reads along the line would end up upside down: its first glyph lies to
/// the right of its last on the screen, as GL JS decides for horizontal text.
pub(crate) fn reads_backwards(first: [f64; 2], last: [f64; 2]) -> bool {
    first[0] > last[0]
}

#[path = "line_glyphs/screen.rs"]
mod screen;
pub(crate) use screen::{place_glyphs_on_screen, OnScreen};

#[cfg(test)]
#[path = "line_glyphs/tests.rs"]
mod tests;
