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

/// The point at arc length `distance` from the start of the polyline and the direction of the
/// segment it lies on; `None` when the line is too short to reach it.
fn point_at(polyline: &[[f32; 2]], distance: f32) -> Option<([f32; 2], f32)> {
    if distance < 0.0 {
        return None;
    }
    let mut travelled = 0.0;
    for pair in polyline.windows(2) {
        let (dx, dy) = (pair[1][0] - pair[0][0], pair[1][1] - pair[0][1]);
        let length = dx.hypot(dy);
        if length > 0.0 && distance <= travelled + length {
            let t = (distance - travelled) / length;
            return Some(([pair[0][0] + dx * t, pair[0][1] + dy * t], dy.atan2(dx)));
        }
        travelled += length;
    }
    None
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
    offsets
        .iter()
        .map(|offset| {
            let along = if flip {
                anchor_distance - offset
            } else {
                anchor_distance + offset
            };
            let (point, angle) = point_at(polyline, along)?;
            Some(GlyphPose {
                point,
                angle: if flip {
                    angle + std::f32::consts::PI
                } else {
                    angle
                },
            })
        })
        .collect()
}

/// Whether text that reads along the line would end up upside down: its first glyph lies to
/// the right of its last on the screen, as GL JS decides for horizontal text.
pub(crate) fn reads_backwards(first: [f64; 2], last: [f64; 2]) -> bool {
    first[0] > last[0]
}

#[cfg(test)]
#[path = "line_glyphs/tests.rs"]
mod tests;
