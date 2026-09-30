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
        });
    }
    poses.into_iter().collect()
}

/// Whether text that reads along the line would end up upside down: its first glyph lies to
/// the right of its last on the screen, as GL JS decides for horizontal text.
pub(crate) fn reads_backwards(first: [f64; 2], last: [f64; 2]) -> bool {
    first[0] > last[0]
}

#[cfg(test)]
#[path = "line_glyphs/tests.rs"]
mod tests;
