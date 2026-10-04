//! Glyph positions along a line as the screen shows it, for labels that turn with their line
//! but stand upright to the viewer: the viewport label plane of GL JS's `placeGlyphAlongLine`.
//!
//! Glyphs are spaced in screen pixels along the projected line, walking outward from the
//! anchor, so a label keeps its pixel size however the line is foreshortened. Each glyph's
//! centre is mapped back onto the line in tile units, where the vertex shader projects it.

use super::GlyphPose;

/// Clip w below which a line vertex counts as behind the eye; the line is cut there.
const NEAR_W: f64 = 1e-3;

/// A point of the line on the screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct OnScreen {
    /// Pixels, y down.
    pub screen: [f64; 2],
    /// Clip-space w, the point's depth in front of the eye.
    pub w: f64,
}

/// A line vertex with where the screen shows it.
#[derive(Clone, Copy)]
struct Vertex {
    tile: [f64; 2],
    at: OnScreen,
}

/// The vertex where the line from `from` toward `tile` crosses [`NEAR_W`], when `tile` itself
/// lies behind the eye.
fn cut_at_eye(
    from: Vertex,
    tile: [f64; 2],
    project: &impl Fn([f64; 2]) -> Option<OnScreen>,
) -> Option<Vertex> {
    let w_at = |t: f64| {
        project([
            from.tile[0] + (tile[0] - from.tile[0]) * t,
            from.tile[1] + (tile[1] - from.tile[1]) * t,
        ])
    };
    let end = w_at(1.0)?;
    // w is affine along the segment in tile space.
    let t = (from.at.w - NEAR_W) / (from.at.w - end.w);
    let point = [
        from.tile[0] + (tile[0] - from.tile[0]) * t,
        from.tile[1] + (tile[1] - from.tile[1]) * t,
    ];
    Some(Vertex {
        tile: point,
        at: project(point)?,
    })
}

/// The segment the anchor lies on and the anchor's own point.
fn anchor_segment(polyline: &[[f32; 2]], anchor_distance: f32) -> Option<(usize, [f64; 2])> {
    let mut travelled = 0.0_f64;
    for (index, pair) in polyline.windows(2).enumerate() {
        let (from, to) = (pair[0].map(f64::from), pair[1].map(f64::from));
        let length = (to[0] - from[0]).hypot(to[1] - from[1]);
        let distance = f64::from(anchor_distance);
        if length > 0.0 && distance <= travelled + length {
            let t = ((distance - travelled) / length).max(0.0);
            return Some((
                index,
                [
                    from[0] + (to[0] - from[0]) * t,
                    from[1] + (to[1] - from[1]) * t,
                ],
            ));
        }
        travelled += length;
    }
    None
}

/// Poses of glyphs whose centres lie `offsets` screen pixels (signed, along the text) from the
/// anchor at `anchor_distance` along `polyline`, as `project` shows the line. `flip` reads the
/// text against the line, each glyph walking the other way and turning half way around. A
/// pose's point is tile units on the line and its angle is the screen direction (y down) of
/// the segment it lies on. `None` when a glyph would lie beyond either end of the visible line.
pub(crate) fn place_glyphs_on_screen(
    polyline: &[[f32; 2]],
    anchor_distance: f32,
    offsets: &[f64],
    flip: bool,
    project: impl Fn([f64; 2]) -> Option<OnScreen>,
) -> Option<Vec<GlyphPose>> {
    let (segment, anchor_tile) = anchor_segment(polyline, anchor_distance)?;
    let anchor = Vertex {
        tile: anchor_tile,
        at: project(anchor_tile).filter(|at| at.w > NEAR_W)?,
    };
    offsets
        .iter()
        .map(|offset| place_one(polyline, segment, anchor, *offset, flip, &project))
        .collect()
}

/// One glyph of [`place_glyphs_on_screen`], walking outward from the anchor.
fn place_one(
    polyline: &[[f32; 2]],
    segment: usize,
    anchor: Vertex,
    offset: f64,
    flip: bool,
    project: &impl Fn([f64; 2]) -> Option<OnScreen>,
) -> Option<GlyphPose> {
    let mut direction: isize = if offset > 0.0 { 1 } else { -1 };
    let mut angle = 0.0;
    if flip {
        direction = -direction;
        angle = std::f64::consts::PI;
    }
    if direction < 0 {
        angle += std::f64::consts::PI;
    }
    let mut index = if direction > 0 {
        segment as isize
    } else {
        segment as isize + 1
    };
    let wanted = offset.abs();
    let (mut previous, mut current) = (anchor, anchor);
    let (mut walked, mut length) = (0.0, 0.0);
    let mut cut = false;
    while walked + length <= wanted {
        index += direction;
        // The line ends, or was cut where it passes behind the eye, before the glyph.
        if cut || index < 0 || index as usize >= polyline.len() {
            return None;
        }
        walked += length;
        previous = current;
        let tile = polyline[index as usize].map(f64::from);
        current = match project(tile).filter(|at| at.w > NEAR_W) {
            Some(at) => Vertex { tile, at },
            None => {
                cut = true;
                cut_at_eye(previous, tile, project)?
            }
        };
        let (dx, dy) = (
            current.at.screen[0] - previous.at.screen[0],
            current.at.screen[1] - previous.at.screen[1],
        );
        length = dx.hypot(dy);
    }
    let s = (wanted - walked) / length;
    // A point a fraction `s` along the segment on the screen lies a perspective-corrected
    // fraction along it in tile space.
    let (w0, w1) = (previous.at.w, current.at.w);
    let t = s * w0 / (s * w0 + (1.0 - s) * w1);
    let point = [
        previous.tile[0] + (current.tile[0] - previous.tile[0]) * t,
        previous.tile[1] + (current.tile[1] - previous.tile[1]) * t,
    ];
    let direction_on_screen = (current.at.screen[1] - previous.at.screen[1])
        .atan2(current.at.screen[0] - previous.at.screen[0]);
    // Within half a turn of zero, the range the line's own direction takes.
    let angle = (angle + direction_on_screen + std::f64::consts::PI)
        .rem_euclid(std::f64::consts::TAU)
        - std::f64::consts::PI;
    Some(GlyphPose {
        point: point.map(|value| value as f32),
        angle: angle as f32,
    })
}

#[cfg(test)]
#[path = "screen/tests.rs"]
mod tests;
