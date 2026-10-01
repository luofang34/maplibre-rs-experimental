//! Keeps the shear of a sharp corner short.
//!
//! The vertices a join adds are displaced along the line, but carry the distance of the corner
//! itself, so a distance-driven dash, pattern or gradient slants across the whole segment that
//! ends in the corner. A point a few units before and after each sharp corner confines the slant
//! to that stretch, as GL JS's line bucket does.

use lyon::{
    math::{point, Point},
    path::{Event, Path},
};

/// Tile units from a sharp corner at which a tile that is not magnified gets a vertex of its own;
/// a magnified tile has proportionally fewer.
pub const UNMAGNIFIED_OFFSET: f32 = 120.0;

/// The distance from a sharp corner for a tile magnified by `overscaling`; GL JS adds no vertex
/// past sixteen times.
pub fn offset_for(overscaling: f64) -> f32 {
    if overscaling > 16.0 {
        0.0
    } else {
        UNMAGNIFIED_OFFSET / overscaling.max(1.0) as f32
    }
}
/// Turns beyond this many degrees count as sharp.
const SHARP_TURN_DEGREES: f32 = 75.0;

/// `path` with a vertex added either side of each sharp corner.
pub fn split(path: &Path, offset: f32) -> Path {
    if offset <= 0.0 {
        return path.clone();
    }
    let mut builder = Path::builder();
    let mut points: Vec<Point> = Vec::new();
    for event in path.iter() {
        match event {
            Event::Begin { at } => {
                points.clear();
                points.push(at);
            }
            Event::Line { to, .. } => points.push(to),
            Event::End { close, .. } => {
                emit(&mut builder, &points, close, offset);
                points.clear();
            }
            Event::Quadratic { .. } | Event::Cubic { .. } => {}
        }
    }
    builder.build()
}

fn emit(builder: &mut lyon::path::path::Builder, points: &[Point], close: bool, offset: f32) {
    let Some(first) = points.first() else {
        return;
    };
    let count = points.len();
    builder.begin(*first);
    let mut last = *first;
    for index in 1..count + usize::from(close) {
        let current = points[index % count];
        let previous = points[index - 1];
        let next = if index + 1 < count + usize::from(close) {
            Some(points[(index + 1) % count])
        } else if close && index + 1 == count + 1 {
            Some(points[1 % count])
        } else {
            None
        };
        let sharp = next.is_some_and(|next| is_sharp(previous, current, next));
        if sharp {
            let before = toward(current, previous, offset);
            if let Some(before) = before {
                builder.line_to(before);
            }
            builder.line_to(current);
            if let Some(after) = next.and_then(|next| toward(current, next, offset)) {
                builder.line_to(after);
                last = after;
                continue;
            }
        } else if !(close && index == count) {
            builder.line_to(current);
        }
        last = current;
    }
    let _ = last;
    builder.end(close);
}

/// The point `offset` from `from` toward `to`, when the segment is long enough to hold two.
fn toward(from: Point, to: Point, offset: f32) -> Option<Point> {
    let delta = to - from;
    let length = delta.length();
    (length > 2.0 * offset).then(|| {
        let step = delta * (offset / length);
        point((from.x + step.x).round(), (from.y + step.y).round())
    })
}

fn is_sharp(previous: Point, current: Point, next: Point) -> bool {
    let (incoming, outgoing) = (current - previous, next - current);
    let lengths = incoming.length() * outgoing.length();
    if lengths <= f32::EPSILON {
        return false;
    }
    let cosine = incoming.dot(outgoing) / lengths;
    cosine < SHARP_TURN_DEGREES.to_radians().cos()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn points_of(path: &Path) -> Vec<Point> {
        path.iter()
            .filter_map(|event| match event {
                Event::Begin { at } => Some(at),
                Event::Line { to, .. } => Some(to),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_sharp_corner_gets_a_vertex_either_side() {
        let mut builder = Path::builder();
        builder.begin(point(0.0, 0.0));
        builder.line_to(point(100.0, 0.0));
        builder.line_to(point(0.0, 10.0));
        builder.end(false);
        let split = points_of(&split(&builder.build(), 7.5));
        assert_eq!(split.len(), 5);
        assert_eq!(split[1], point(93.0, 0.0));
        assert_eq!(split[2], point(100.0, 0.0));
    }

    #[test]
    fn a_gentle_corner_is_left_alone() {
        let mut builder = Path::builder();
        builder.begin(point(0.0, 0.0));
        builder.line_to(point(100.0, 0.0));
        builder.line_to(point(200.0, 20.0));
        builder.end(false);
        assert_eq!(points_of(&split(&builder.build(), 7.5)).len(), 3);
    }

    #[test]
    fn the_offset_shrinks_with_the_magnification_and_vanishes_past_sixteen() {
        assert_eq!(offset_for(1.0), 120.0);
        assert_eq!(offset_for(16.0), 7.5);
        assert_eq!(offset_for(32.0), 0.0);
    }
}
