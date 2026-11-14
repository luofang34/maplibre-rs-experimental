//! Ramer-Douglas-Peucker simplification in `[0..1]` world Mercator coordinates.

use super::{Geometry, Point, Ring};

/// Squared world-space Douglas-Peucker tolerance at `zoom` for a pixel `tolerance` on a 512-px tile.
pub(super) fn squared_world_tolerance(tolerance: f64, zoom: u32) -> f64 {
    if tolerance <= 0.0 || zoom >= 24 {
        return 0.0;
    }
    let scale = 512.0 * f64::from(1_u32 << zoom);
    (tolerance / scale).powi(2)
}

/// Simplifies a polyline or ring with squared world-space tolerance `sq_tol`, always preserving
/// the first and last vertices.
pub(super) fn simplify_points(points: &[Point], sq_tol: f64) -> Ring {
    if points.len() <= 2 || sq_tol <= 0.0 {
        return points.to_vec();
    }
    let len = points.len();
    let mut keep = vec![false; len];
    keep[0] = true;
    keep[len - 1] = true;

    let mut stack = vec![(0_usize, len - 1)];
    while let Some((first, last)) = stack.pop() {
        if last <= first + 1 {
            continue;
        }
        let mut max_sq_dist = sq_tol;
        let mut split_index = None;
        let a = points[first];
        let b = points[last];
        for (index, &point) in points.iter().enumerate().take(last).skip(first + 1) {
            let sq_dist = sq_seg_dist(point, a, b);
            if sq_dist > max_sq_dist {
                max_sq_dist = sq_dist;
                split_index = Some(index);
            }
        }
        if let Some(index) = split_index {
            keep[index] = true;
            stack.push((first, index));
            stack.push((index, last));
        }
    }

    points
        .iter()
        .zip(keep)
        .filter_map(|(&point, retained)| retained.then_some(point))
        .collect()
}

/// Simplifies `geometry`'s lines and polygon rings in place for tile `zoom`.
pub(super) fn simplify_geometry(geometry: &mut Geometry, tolerance: f64, zoom: u32) {
    let sq_tol = squared_world_tolerance(tolerance, zoom);
    if sq_tol <= 0.0 {
        return;
    }
    for line in &mut geometry.lines {
        *line = simplify_points(line, sq_tol);
    }
    for polygon in &mut geometry.polygons {
        for ring in polygon {
            *ring = simplify_points(ring, sq_tol);
        }
    }
}

fn sq_seg_dist(p: Point, a: Point, b: Point) -> f64 {
    let mut x = a[0];
    let mut y = a[1];
    let dx = b[0] - x;
    let dy = b[1] - y;
    if dx != 0.0 || dy != 0.0 {
        let t = ((p[0] - x) * dx + (p[1] - y) * dy) / (dx * dx + dy * dy);
        if t > 1.0 {
            x = b[0];
            y = b[1];
        } else if t > 0.0 {
            x += dx * t;
            y += dy * t;
        }
    }
    let ex = p[0] - x;
    let ey = p[1] - y;
    ex * ex + ey * ey
}
