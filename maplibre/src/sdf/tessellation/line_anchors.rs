//! Anchors for labels that follow a line: evenly spaced along it, kept off sharp bends, and
//! offset so the repeats of a line that crosses tiles stay consistent.
//!
//! Lengths are tile units; the caller converts pixels with the tile's pixel ratio.

use crate::coords::EXTENT;

type Point = [f64; 2];

/// A place a line label can sit, on the segment `segment` of its polyline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct LineAnchor {
    pub point: Point,
    /// Direction of the segment the anchor lies on, in radians.
    pub angle: f64,
    /// Index of the segment's first vertex.
    pub segment: usize,
}

/// What decides where labels of one feature may go.
#[derive(Clone, Copy, Debug)]
pub(super) struct AnchorSpacing {
    /// Distance between repeats.
    pub spacing: f64,
    /// Bend a label may take within the window of a few glyphs, in radians.
    pub max_angle: f64,
    /// Length of the label along the line.
    pub label_length: f64,
    /// Height of a glyph, which sizes the bend window and the start offset.
    pub text_size: f64,
}

fn distance(a: Point, b: Point) -> f64 {
    (b[0] - a[0]).hypot(b[1] - a[1])
}

fn direction(from: Point, to: Point) -> f64 {
    (to[1] - from[1]).atan2(to[0] - from[0])
}

fn length(line: &[Point]) -> f64 {
    line.windows(2)
        .map(|segment| distance(segment[0], segment[1]))
        .sum()
}

/// The parts of `lines` inside the tile square.
pub(super) fn clip_to_tile(lines: &[Vec<Point>]) -> Vec<Vec<Point>> {
    let extent = EXTENT;
    let mut clipped: Vec<Vec<Point>> = Vec::new();
    for line in lines {
        let mut current: Option<usize> = None;
        for pair in line.windows(2) {
            let (mut from, mut to) = (pair[0], pair[1]);
            if !clip_segment(&mut from, &mut to, extent) {
                continue;
            }
            let continues = current
                .and_then(|index| clipped[index].last())
                .is_some_and(|last| *last == from);
            if !continues {
                clipped.push(vec![from]);
                current = Some(clipped.len() - 1);
            }
            if let Some(index) = current {
                clipped[index].push(to);
            }
        }
    }
    clipped
}

/// Trims a segment to the square `0..extent` in each axis; false when nothing remains.
fn clip_segment(from: &mut Point, to: &mut Point, extent: f64) -> bool {
    for axis in 0..2 {
        for bound in [0.0, extent] {
            let inside = |value: f64| {
                if bound == 0.0 {
                    value >= bound
                } else {
                    value < bound
                }
            };
            let (a_in, b_in) = (inside(from[axis]), inside(to[axis]));
            if !a_in && !b_in {
                return false;
            }
            if a_in != b_in {
                let t = (bound - from[axis]) / (to[axis] - from[axis]);
                let cut = |a: Point, b: Point| {
                    [
                        (a[0] + (b[0] - a[0]) * t).round(),
                        (a[1] + (b[1] - a[1]) * t).round(),
                    ]
                };
                if a_in {
                    *to = cut(*from, *to);
                } else {
                    *from = cut(*from, *to);
                }
            }
        }
    }
    true
}

/// Anchors spaced along one polyline, or none when the label cannot fit anywhere.
pub(super) fn line_anchors(line: &[Point], params: AnchorSpacing) -> Vec<LineAnchor> {
    let extent = EXTENT;
    let on_edge = line.first().is_some_and(|start| {
        start[0] == 0.0 || start[0] == extent || start[1] == 0.0 || start[1] == extent
    });
    let mut spacing = params.spacing;
    if spacing - params.label_length < spacing / 4.0 {
        spacing = params.label_length + spacing / 4.0;
    }
    // Repeats begin at a fixed distance from the tile edge, so the neighbouring tile's
    // continuation of the line places its labels on the same rhythm.
    let offset = if on_edge {
        params.text_size % spacing
    } else {
        (params.label_length / 2.0 + 2.0 * params.text_size) % spacing
    };
    resample(line, offset, spacing, params, on_edge, false)
}

fn resample(
    line: &[Point],
    offset: f64,
    spacing: f64,
    params: AnchorSpacing,
    on_edge: bool,
    at_middle: bool,
) -> Vec<LineAnchor> {
    let half_label = params.label_length / 2.0;
    let line_length = length(line);
    let window = 0.6 * params.text_size;
    let extent = EXTENT;
    let (mut travelled, mut marked) = (0.0, offset - spacing);
    let mut anchors = Vec::new();
    for (index, pair) in line.windows(2).enumerate() {
        let segment = distance(pair[0], pair[1]);
        let angle = direction(pair[0], pair[1]);
        while marked + spacing < travelled + segment {
            marked += spacing;
            let t = (marked - travelled) / segment;
            let point = [
                (pair[0][0] + (pair[1][0] - pair[0][0]) * t).round(),
                (pair[0][1] + (pair[1][1] - pair[0][1]) * t).round(),
            ];
            let fits = (0.0..extent).contains(&point[0])
                && (0.0..extent).contains(&point[1])
                && marked - half_label >= 0.0
                && marked + half_label <= line_length;
            let anchor = LineAnchor {
                point,
                angle,
                segment: index,
            };
            if fits
                && bends_within_limit(line, anchor, params.label_length, window, params.max_angle)
            {
                anchors.push(anchor);
            }
        }
        travelled += segment;
    }
    if !at_middle && anchors.is_empty() && !on_edge {
        // No anchor fit at the regular rhythm, so try one at the middle of the line.
        return resample(line, travelled / 2.0, spacing, params, on_edge, true);
    }
    anchors
}

/// The anchor of a `line-center` label: the middle of the line, when the label fits there.
pub(super) fn center_anchor(line: &[Point], params: AnchorSpacing) -> Option<LineAnchor> {
    let total = length(line);
    let mut remaining = total / 2.0;
    for (index, pair) in line.windows(2).enumerate() {
        let segment = distance(pair[0], pair[1]);
        if remaining <= segment && segment > 0.0 {
            let t = remaining / segment;
            let anchor = LineAnchor {
                point: [
                    (pair[0][0] + (pair[1][0] - pair[0][0]) * t).round(),
                    (pair[0][1] + (pair[1][1] - pair[0][1]) * t).round(),
                ],
                angle: direction(pair[0], pair[1]),
                segment: index,
            };
            let window = 0.6 * params.text_size;
            return bends_within_limit(line, anchor, params.label_length, window, params.max_angle)
                .then_some(anchor);
        }
        remaining -= segment;
    }
    None
}

/// Whether the line under a label centred on `anchor` bends less than `max_angle` within any
/// stretch of `window`, and is long enough on both sides.
fn bends_within_limit(
    line: &[Point],
    anchor: LineAnchor,
    label_length: f64,
    window: f64,
    max_angle: f64,
) -> bool {
    let mut point = anchor.point;
    let mut index = anchor.segment as isize + 1;
    let mut anchor_distance = 0.0;
    // Walk back to the segment the label starts on.
    while anchor_distance > -label_length / 2.0 {
        index -= 1;
        if index < 0 {
            return false;
        }
        anchor_distance -= distance(line[index as usize], point);
        point = line[index as usize];
    }
    let mut index = index as usize;
    anchor_distance += distance(line[index], line[index + 1]);
    index += 1;
    let mut recent: Vec<(f64, f64)> = Vec::new();
    let mut recent_angle = 0.0;
    while anchor_distance < label_length / 2.0 {
        let (Some(previous), Some(current)) = (line.get(index - 1), line.get(index)) else {
            return false;
        };
        let Some(next) = line.get(index + 1) else {
            return false;
        };
        let delta = direction(*previous, *current) - direction(*current, *next);
        let delta = (((delta + 3.0 * std::f64::consts::PI) % (2.0 * std::f64::consts::PI))
            - std::f64::consts::PI)
            .abs();
        recent.push((anchor_distance, delta));
        recent_angle += delta;
        while recent
            .first()
            .is_some_and(|(at, _)| anchor_distance - at > window)
        {
            recent_angle -= recent.remove(0).1;
        }
        if recent_angle > max_angle {
            return false;
        }
        index += 1;
        anchor_distance += distance(*current, *next);
    }
    true
}

#[cfg(test)]
#[path = "line_anchors/tests.rs"]
mod tests;
