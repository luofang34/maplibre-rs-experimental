//! Ring extraction, hole classification and tile-box clipping for extruded polygons.

use lyon::path::{Event, Path};

use crate::coords::EXTENT;

pub(super) type Ring = Vec<[f32; 2]>;

pub(super) fn rings(path: &Path) -> Vec<Ring> {
    let mut rings = Vec::new();
    let mut current = Ring::new();
    for event in path.iter() {
        match event {
            Event::Begin { at } => current = vec![at.to_array()],
            Event::Line { to, .. } => current.push(to.to_array()),
            Event::End { .. } => rings.push(std::mem::take(&mut current)),
            Event::Quadratic { to, .. } | Event::Cubic { to, .. } => current.push(to.to_array()),
        }
    }
    rings
}

/// The sum of `(x2 - x1) * (y1 + y2)` over the ring's edges, as geojson-vt measures the winding.
pub(super) fn trapezoid_area(ring: &Ring) -> f32 {
    ring.iter()
        .zip(ring.iter().cycle().skip(1))
        .map(|(a, b)| (b[0] - a[0]) * (a[1] + b[1]))
        .sum()
}

/// Twice the signed area in the raw coordinates; the sign tells which side the inside is on.
pub(super) fn signed_area(ring: &Ring) -> f32 {
    ring.iter()
        .zip(ring.iter().cycle().skip(1))
        .map(|(a, b)| a[0] * b[1] - b[0] * a[1])
        .sum()
}

fn contains(ring: &Ring, [x, y]: [f32; 2]) -> bool {
    let mut inside = false;
    for (a, b) in ring.iter().zip(ring.iter().cycle().skip(1)) {
        if (a[1] > y) != (b[1] > y) && x < (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
    }
    inside
}

/// A ring inside an odd number of others is a hole.
pub(super) fn is_hole(index: usize, rings: &[Ring]) -> bool {
    if rings.len() <= 1 {
        return false;
    }
    let Some(&point) = rings[index].first() else {
        return false;
    };
    rings
        .iter()
        .enumerate()
        .filter(|(other, ring)| *other != index && contains(ring, point))
        .count()
        % 2
        == 1
}

/// Walls along a tile's clipped edge would show a seam between tiles. When `include_tile_boundary`
/// is true (polygons clipped tightly to `[0, EXTENT]` for translucent volumes), edges lying right
/// on `0` or `EXTENT` are also omitted so adjacent tiles share no internal vertical curtain.
pub(super) fn on_clipped_edge(a: [f32; 2], b: [f32; 2], include_tile_boundary: bool) -> bool {
    let extent = EXTENT as f32;
    if include_tile_boundary {
        (a[0] == b[0] && (a[0] <= 0.0 || a[0] >= extent))
            || (a[1] == b[1] && (a[1] <= 0.0 || a[1] >= extent))
    } else {
        (a[0] == b[0] && (a[0] < 0.0 || a[0] > extent))
            || (a[1] == b[1] && (a[1] < 0.0 || a[1] > extent))
    }
}

/// `ring` cut to the square from `low` to `high` on both axes, one edge of the square at a time.
fn clip_ring(ring: &Ring, (low, high): (f32, f32)) -> Ring {
    let mut clipped = ring.clone();
    for axis in 0..2 {
        for keep_above in [true, false] {
            let bound = if keep_above { low } else { high };
            let inside = |point: &[f32; 2]| {
                if keep_above {
                    point[axis] >= bound
                } else {
                    point[axis] <= bound
                }
            };
            let mut next = Ring::new();
            for (a, b) in clipped.iter().zip(clipped.iter().cycle().skip(1)) {
                if inside(a) {
                    next.push(*a);
                }
                if inside(a) != inside(b) {
                    let t = (bound - a[axis]) / (b[axis] - a[axis]);
                    let mut crossing = [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])];
                    crossing[axis] = bound;
                    next.push(crossing);
                }
            }
            clipped = next;
        }
    }
    clipped
}

/// `path` with each ring clipped to `buffer` tile units past the tile and wound the way a
/// GeoJSON source winds them, outer rings counterclockwise on the map and holes the other way;
/// a ring left with no area is dropped. The wall distance of a pattern runs along that winding.
pub(in crate::vector::tessellation) fn clipped(path: &Path, buffer: f32) -> Path {
    let bounds = (-buffer, EXTENT as f32 + buffer);
    let rings: Vec<Ring> = rings(path)
        .iter()
        .map(|ring| clip_ring(ring, bounds))
        .filter(|ring| ring.len() >= 3 && signed_area(ring) != 0.0)
        .collect();
    build_wound_rings(&rings)
}

/// Normalizes ring winding without clipping so outer walls and hole walls wind consistently for
/// back/front face culling.
pub(super) fn normalized_rings(rings: &[Ring]) -> Vec<Ring> {
    rings
        .iter()
        .enumerate()
        .filter(|(_, ring)| ring.len() >= 3 && signed_area(ring) != 0.0)
        .map(|(index, ring)| {
            let mut ring = ring.clone();
            if (trapezoid_area(&ring) > 0.0) != is_hole(index, rings) {
                ring[1..].reverse();
            }
            ring
        })
        .collect()
}

fn build_wound_rings(rings: &[Ring]) -> Path {
    let mut builder = Path::builder();
    for ring in normalized_rings(rings) {
        builder.begin(lyon::math::point(ring[0][0], ring[0][1]));
        for point in &ring[1..] {
            builder.line_to(lyon::math::point(point[0], point[1]));
        }
        builder.end(true);
    }
    builder.build()
}
