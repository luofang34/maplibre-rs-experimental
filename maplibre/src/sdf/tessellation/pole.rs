//! The pole of inaccessibility of a polygon: the point farthest from its edges, where GL JS
//! puts the label of a polygon. Finds it by splitting cells over the polygon's bounding box and
//! keeping those that could still beat the best, as the `polylabel` algorithm does.

use std::{cmp::Ordering, collections::BinaryHeap};

type Ring = Vec<[f64; 2]>;

struct Cell {
    x: f64,
    y: f64,
    half: f64,
    /// Distance from the cell centre to the polygon's edge, negative outside it.
    distance: f64,
    /// The most any point of the cell can reach.
    potential: f64,
}

impl Cell {
    fn new(x: f64, y: f64, half: f64, rings: &[Ring]) -> Self {
        let distance = signed_distance(x, y, rings);
        Self {
            x,
            y,
            half,
            distance,
            potential: distance + half * std::f64::consts::SQRT_2,
        }
    }
}

impl PartialEq for Cell {
    fn eq(&self, other: &Self) -> bool {
        self.potential == other.potential
    }
}
impl Eq for Cell {}
impl PartialOrd for Cell {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Cell {
    fn cmp(&self, other: &Self) -> Ordering {
        self.potential.total_cmp(&other.potential)
    }
}

fn segment_distance_squared(x: f64, y: f64, a: [f64; 2], b: [f64; 2]) -> f64 {
    let (mut px, mut py) = (a[0], a[1]);
    let (dx, dy) = (b[0] - px, b[1] - py);
    if dx != 0.0 || dy != 0.0 {
        let t = ((x - px) * dx + (y - py) * dy) / (dx * dx + dy * dy);
        if t > 1.0 {
            px = b[0];
            py = b[1];
        } else if t > 0.0 {
            px += dx * t;
            py += dy * t;
        }
    }
    (x - px).powi(2) + (y - py).powi(2)
}

/// Distance from a point to the nearest edge of the rings, positive inside the polygon.
fn signed_distance(x: f64, y: f64, rings: &[Ring]) -> f64 {
    let mut inside = false;
    let mut nearest = f64::INFINITY;
    for ring in rings {
        let mut previous = ring.len().saturating_sub(1);
        for (index, a) in ring.iter().enumerate() {
            let b = ring[previous];
            if (a[1] > y) != (b[1] > y) && x < (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]) + a[0] {
                inside = !inside;
            }
            nearest = nearest.min(segment_distance_squared(x, y, *a, b));
            previous = index;
        }
    }
    if nearest == 0.0 {
        0.0
    } else if inside {
        nearest.sqrt()
    } else {
        -nearest.sqrt()
    }
}

/// The cell at the centroid of the outer ring, a good first guess.
fn centroid_cell(rings: &[Ring]) -> Cell {
    let ring = &rings[0];
    let (mut area, mut x, mut y) = (0.0, 0.0, 0.0);
    let mut previous = ring.len() - 1;
    for (index, a) in ring.iter().enumerate() {
        let b = ring[previous];
        let f = a[0] * b[1] - b[0] * a[1];
        x += (a[0] + b[0]) * f;
        y += (a[1] + b[1]) * f;
        area += f * 3.0;
        previous = index;
    }
    if area == 0.0 {
        return Cell::new(ring[0][0], ring[0][1], 0.0, rings);
    }
    Cell::new(x / area, y / area, 0.0, rings)
}

/// The point of the polygon farthest from its edges, to within `precision`.
pub(super) fn pole_of_inaccessibility(rings: &[Ring], precision: f64) -> Option<[f64; 2]> {
    let outer = rings.first().filter(|ring| ring.len() > 2)?;
    let (mut min_x, mut min_y) = (f64::INFINITY, f64::INFINITY);
    let (mut max_x, mut max_y) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for point in outer {
        min_x = min_x.min(point[0]);
        min_y = min_y.min(point[1]);
        max_x = max_x.max(point[0]);
        max_y = max_y.max(point[1]);
    }
    let (width, height) = (max_x - min_x, max_y - min_y);
    let cell_size = width.min(height);
    if cell_size == 0.0 {
        return Some([min_x, min_y]);
    }
    let mut half = cell_size / 2.0;
    let mut queue = BinaryHeap::new();
    let mut x = min_x;
    while x < max_x {
        let mut y = min_y;
        while y < max_y {
            queue.push(Cell::new(x + half, y + half, half, rings));
            y += cell_size;
        }
        x += cell_size;
    }
    let mut best = centroid_cell(rings);
    let bounding = Cell::new(min_x + width / 2.0, min_y + height / 2.0, 0.0, rings);
    if bounding.distance > best.distance {
        best = bounding;
    }
    while let Some(cell) = queue.pop() {
        if cell.distance > best.distance {
            best = Cell {
                x: cell.x,
                y: cell.y,
                half: cell.half,
                distance: cell.distance,
                potential: cell.potential,
            };
        }
        if cell.potential - best.distance <= precision {
            continue;
        }
        half = cell.half / 2.0;
        for (dx, dy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
            queue.push(Cell::new(
                cell.x + dx * half,
                cell.y + dy * half,
                half,
                rings,
            ));
        }
    }
    Some([best.x, best.y])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pole_of_a_square_is_its_centre_and_of_an_l_its_thick_corner() {
        let square = vec![vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]];
        let [x, y] = pole_of_inaccessibility(&square, 0.1).expect("pole");
        assert!((x - 5.0).abs() < 0.2 && (y - 5.0).abs() < 0.2);
        // An L whose corner block is the widest part.
        let l_shape = vec![vec![
            [0.0, 0.0],
            [10.0, 0.0],
            [10.0, 2.0],
            [2.0, 2.0],
            [2.0, 10.0],
            [0.0, 10.0],
        ]];
        let [x, y] = pole_of_inaccessibility(&l_shape, 0.05).expect("pole");
        assert!(x < 2.5 && y < 2.5, "({x}, {y})");
    }
}
