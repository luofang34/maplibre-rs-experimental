//! Cutting a tile from the last tile a source has above it.
//!
//! A source whose tiles end at a zoom is drawn at finer zooms from its tile at that zoom, as GL
//! JS draws an overscaled tile. A tile here holds every source at once, so instead the
//! ancestor's features are brought into the finer tile's own grid, scaled and shifted, with what
//! lies beyond the tile and its buffer cut away; the tile is then drawn at its own zoom like the
//! tiles of the sources that reach it.

use geozero::mvt::{tile, Tile};

use crate::coords::WorldTileCoords;

const MOVE_TO: u32 = 1;
const LINE_TO: u32 = 2;
const CLOSE_PATH: u32 = 7;

/// The part of the extent kept around the tile, as GL JS keeps 128 of 4096.
const BUFFER_DIVISOR: i64 = 32;

type Point = (i64, i64);

/// Brings the features of `tile`, cut for `from`, into the grid of its descendant `to`.
pub(super) fn into_descendant(tile: &mut Tile, from: WorldTileCoords, to: WorldTileCoords) {
    let levels = u32::from(u8::from(to.z).saturating_sub(u8::from(from.z))).min(31);
    if levels == 0 {
        return;
    }
    let scale = 1_i64 << levels;
    let column = i64::from(to.x) - (i64::from(from.x) << levels);
    let row = i64::from(to.y) - (i64::from(from.y) << levels);
    for layer in &mut tile.layers {
        let extent = i64::from(layer.extent.unwrap_or(4096)).max(1);
        let frame = Frame {
            scale,
            offset: (column * extent, row * extent),
            low: -extent / BUFFER_DIVISOR,
            high: extent + extent / BUFFER_DIVISOR,
        };
        layer.features.retain_mut(|feature| frame.cut(feature));
    }
}

/// The descendant's grid seen from the ancestor's, and the box kept of it.
struct Frame {
    scale: i64,
    offset: Point,
    low: i64,
    high: i64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Point,
    Line,
    Polygon,
}

impl Kind {
    fn of(feature: &tile::Feature) -> Option<Self> {
        match feature.r#type? {
            1 => Some(Self::Point),
            2 => Some(Self::Line),
            3 => Some(Self::Polygon),
            _ => None,
        }
    }
}

impl Frame {
    /// Rescales the feature in place; `false` when nothing of it lies in the frame.
    fn cut(&self, feature: &mut tile::Feature) -> bool {
        let Some(kind) = Kind::of(feature) else {
            return false;
        };
        let Some(parts) = decode(&feature.geometry) else {
            return false;
        };
        let parts: Vec<Vec<Point>> = parts
            .into_iter()
            .map(|part| part.into_iter().map(|point| self.place(point)).collect())
            .collect();
        let kept = match kind {
            Kind::Point => self.points(parts),
            Kind::Line => self.lines(parts),
            Kind::Polygon => self.polygons(parts),
        };
        if kept.is_empty() {
            return false;
        }
        feature.geometry = encode(kind, &kept);
        true
    }

    /// Saturates rather than overflows for a malformed tile whose deltas run far past its
    /// extent; what saturates lies far outside the frame and is cut away.
    fn place(&self, (x, y): Point) -> Point {
        (
            x.saturating_mul(self.scale).saturating_sub(self.offset.0),
            y.saturating_mul(self.scale).saturating_sub(self.offset.1),
        )
    }

    fn inside(&self, (x, y): Point) -> bool {
        (self.low..=self.high).contains(&x) && (self.low..=self.high).contains(&y)
    }

    fn points(&self, parts: Vec<Vec<Point>>) -> Vec<Vec<Point>> {
        let kept: Vec<Point> = parts
            .into_iter()
            .flatten()
            .filter(|point| self.inside(*point))
            .collect();
        if kept.is_empty() {
            Vec::new()
        } else {
            vec![kept]
        }
    }

    fn lines(&self, parts: Vec<Vec<Point>>) -> Vec<Vec<Point>> {
        let mut out = Vec::new();
        for part in parts {
            let mut current: Vec<Point> = Vec::new();
            for pair in part.windows(2) {
                let Some((start, end, entered, left)) = self.clip_segment(pair[0], pair[1]) else {
                    flush(&mut out, &mut current);
                    continue;
                };
                if entered || current.is_empty() {
                    flush(&mut out, &mut current);
                    current.push(start);
                }
                push_distinct(&mut current, end);
                if left {
                    flush(&mut out, &mut current);
                }
            }
            flush(&mut out, &mut current);
        }
        out
    }

    /// The part of the segment in the box, by Liang–Barsky, with whether the segment entered
    /// the box on its way and whether it left it.
    fn clip_segment(&self, from: Point, to: Point) -> Option<(Point, Point, bool, bool)> {
        let (dx, dy) = ((to.0 - from.0) as f64, (to.1 - from.1) as f64);
        let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
        let edges = [
            (-dx, (from.0 - self.low) as f64),
            (dx, (self.high - from.0) as f64),
            (-dy, (from.1 - self.low) as f64),
            (dy, (self.high - from.1) as f64),
        ];
        for (p, q) in edges {
            if p == 0.0 {
                if q < 0.0 {
                    return None;
                }
                continue;
            }
            let t = q / p;
            if p < 0.0 {
                if t > t1 {
                    return None;
                }
                t0 = t0.max(t);
            } else {
                if t < t0 {
                    return None;
                }
                t1 = t1.min(t);
            }
        }
        let at = |t: f64| {
            (
                (from.0 as f64 + dx * t).round() as i64,
                (from.1 as f64 + dy * t).round() as i64,
            )
        };
        Some((at(t0), at(t1), t0 > 0.0, t1 < 1.0))
    }

    /// Clips each ring to the box; the holes of an exterior ring the box leaves out go with it.
    fn polygons(&self, parts: Vec<Vec<Point>>) -> Vec<Vec<Point>> {
        let mut out = Vec::new();
        let mut exterior_kept = false;
        for ring in parts {
            let exterior = signed_area(&ring) > 0;
            if !exterior && !exterior_kept {
                continue;
            }
            let clipped = self.clip_ring(ring);
            // Rounding at the box can turn a sliver's area over; a ring that changed its turn
            // would be read as the other kind of ring, so it goes.
            let kept = clipped.len() >= 3 && (signed_area(&clipped) > 0) == exterior;
            if exterior {
                exterior_kept = kept;
            }
            if kept {
                out.push(clipped);
            }
        }
        out
    }

    /// Sutherland–Hodgman against the four sides of the box.
    fn clip_ring(&self, mut ring: Vec<Point>) -> Vec<Point> {
        for edge in 0..4 {
            if ring.is_empty() {
                break;
            }
            let mut out: Vec<Point> = Vec::new();
            let count = ring.len();
            for index in 0..count {
                let current = ring[index];
                let previous = ring[(index + count - 1) % count];
                let (current_in, previous_in) =
                    (self.within(edge, current), self.within(edge, previous));
                if current_in {
                    if !previous_in {
                        push_distinct(&mut out, self.intersect(edge, previous, current));
                    }
                    push_distinct(&mut out, current);
                } else if previous_in {
                    push_distinct(&mut out, self.intersect(edge, previous, current));
                }
            }
            if out.len() > 1 && out.first() == out.last() {
                out.pop();
            }
            ring = out;
        }
        ring
    }

    fn within(&self, edge: usize, (x, y): Point) -> bool {
        match edge {
            0 => x >= self.low,
            1 => x <= self.high,
            2 => y >= self.low,
            _ => y <= self.high,
        }
    }

    /// Where the segment crosses the side, which it does since one end is within and one is not.
    fn intersect(&self, edge: usize, from: Point, to: Point) -> Point {
        let along = |k: i64, a: i64, b: i64, c: i64, d: i64| {
            let t = (k - a) as f64 / (b - a) as f64;
            (c as f64 + (d - c) as f64 * t).round() as i64
        };
        match edge {
            0 => (self.low, along(self.low, from.0, to.0, from.1, to.1)),
            1 => (self.high, along(self.high, from.0, to.0, from.1, to.1)),
            2 => (along(self.low, from.1, to.1, from.0, to.0), self.low),
            _ => (along(self.high, from.1, to.1, from.0, to.0), self.high),
        }
    }
}

fn push_distinct(part: &mut Vec<Point>, point: Point) {
    if part.last() != Some(&point) {
        part.push(point);
    }
}

/// Moves a part of two or more points into `out`, and drops a shorter one.
fn flush(out: &mut Vec<Vec<Point>>, current: &mut Vec<Point>) {
    let part = std::mem::take(current);
    if part.len() >= 2 {
        out.push(part);
    }
}

/// Twice the shoelace area, positive for the clockwise rings of a y-down grid.
fn signed_area(ring: &[Point]) -> i128 {
    let mut area = 0_i128;
    for (index, point) in ring.iter().enumerate() {
        let next = ring[(index + 1) % ring.len()];
        area += i128::from(point.0) * i128::from(next.1) - i128::from(next.0) * i128::from(point.1);
    }
    area
}

fn zigzag_decode(value: u32) -> i64 {
    i64::from((value >> 1) as i32 ^ -((value & 1) as i32))
}

fn zigzag(value: i64) -> u32 {
    let value = value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    ((value << 1) ^ (value >> 31)) as u32
}

/// The parts of a command stream as absolute points: the points of a point feature, the lines
/// of a line feature, or the rings of a polygon feature. `None` for a stream that is not one.
fn decode(geometry: &[u32]) -> Option<Vec<Vec<Point>>> {
    let mut parts = Vec::new();
    let mut current: Vec<Point> = Vec::new();
    let mut cursor = (0_i64, 0_i64);
    let mut index = 0;
    while index < geometry.len() {
        let word = geometry[index];
        index += 1;
        let (id, count) = (word & 7, (word >> 3) as usize);
        match id {
            MOVE_TO | LINE_TO => {
                if id == MOVE_TO && !current.is_empty() {
                    parts.push(std::mem::take(&mut current));
                }
                for _ in 0..count {
                    let dx = zigzag_decode(*geometry.get(index)?);
                    let dy = zigzag_decode(*geometry.get(index + 1)?);
                    index += 2;
                    cursor = (cursor.0 + dx, cursor.1 + dy);
                    current.push(cursor);
                }
            }
            CLOSE_PATH => {
                if !current.is_empty() {
                    parts.push(std::mem::take(&mut current));
                }
            }
            _ => return None,
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    Some(parts)
}

fn encode(kind: Kind, parts: &[Vec<Point>]) -> Vec<u32> {
    let mut out = Vec::new();
    let mut cursor = (0_i64, 0_i64);
    match kind {
        Kind::Point => {
            let points: Vec<Point> = parts.iter().flatten().copied().collect();
            out.push(command(MOVE_TO, points.len()));
            for point in points {
                step(&mut out, &mut cursor, point);
            }
        }
        Kind::Line => {
            for part in parts {
                path(&mut out, &mut cursor, part, false);
            }
        }
        Kind::Polygon => {
            for part in parts {
                path(&mut out, &mut cursor, part, true);
            }
        }
    }
    out
}

fn command(id: u32, count: usize) -> u32 {
    ((count as u32) << 3) | id
}

fn step(out: &mut Vec<u32>, cursor: &mut Point, to: Point) {
    out.push(zigzag(to.0 - cursor.0));
    out.push(zigzag(to.1 - cursor.1));
    *cursor = to;
}

fn path(out: &mut Vec<u32>, cursor: &mut Point, points: &[Point], close: bool) {
    let Some((first, rest)) = points.split_first() else {
        return;
    };
    out.push(command(MOVE_TO, 1));
    step(out, cursor, *first);
    out.push(command(LINE_TO, rest.len()));
    for point in rest {
        step(out, cursor, *point);
    }
    if close {
        out.push(command(CLOSE_PATH, 1));
    }
}

#[cfg(test)]
mod tests;
