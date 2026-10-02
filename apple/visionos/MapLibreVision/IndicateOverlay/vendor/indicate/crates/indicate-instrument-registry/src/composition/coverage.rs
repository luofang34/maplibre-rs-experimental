//! Whether a union of rectangles wholly covers another — the dead-slot
//! question, answered without allocating.
//!
//! The union's edges cut the target into a grid of cells, and a union
//! of axis-aligned rectangles covers the target exactly when it covers
//! every cell. The grid is bounded by
//! [`crate::MAX_COMPOSITION_SLOTS`], so the working arrays are fixed.

use indicate_instrument_descriptor::Region;

use crate::composition::MAX_COMPOSITION_SLOTS;

/// Edges a compressed axis can carry: the target's two, plus two per
/// covering rectangle.
const MAX_EDGES: usize = 2 + 2 * MAX_COMPOSITION_SLOTS;

/// Whether `covers` together contain every point of `target`.
pub(super) fn covered(target: &Region, covers: &[Region]) -> bool {
    let mut xs = [0.0f32; MAX_EDGES];
    let mut ys = [0.0f32; MAX_EDGES];
    let x_count = compress(&mut xs, target.x, target.right(), covers, Axis::X);
    let y_count = compress(&mut ys, target.y, target.bottom(), covers, Axis::Y);
    for xi in 1..x_count {
        for yi in 1..y_count {
            let (Some(x0), Some(x1)) = (xs.get(xi - 1), xs.get(xi)) else {
                return false;
            };
            let (Some(y0), Some(y1)) = (ys.get(yi - 1), ys.get(yi)) else {
                return false;
            };
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            let point = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
            if !covers.iter().any(|cover| holds(cover, point)) {
                return false;
            }
        }
    }
    // A target with no interior cell has no interior to leave showing.
    x_count > 1 && y_count > 1
}

enum Axis {
    X,
    Y,
}

/// Fills `out` with the sorted, deduplicated cut points between `low`
/// and `high`, and returns how many it wrote.
fn compress(
    out: &mut [f32; MAX_EDGES],
    low: f32,
    high: f32,
    covers: &[Region],
    axis: Axis,
) -> usize {
    let mut count = push(out, 0, low);
    count = push(out, count, high);
    for cover in covers {
        let (near, far) = match axis {
            Axis::X => (cover.x, cover.right()),
            Axis::Y => (cover.y, cover.bottom()),
        };
        // Edges outside the target cut nothing inside it.
        if near >= low && near <= high {
            count = push(out, count, near);
        }
        if far >= low && far <= high {
            count = push(out, count, far);
        }
    }
    let edges = out.get_mut(..count).unwrap_or(&mut []);
    edges.sort_unstable_by(f32::total_cmp);
    dedup(edges)
}

fn push(out: &mut [f32; MAX_EDGES], count: usize, value: f32) -> usize {
    match out.get_mut(count) {
        Some(slot) => {
            *slot = value;
            count.wrapping_add(1)
        }
        None => count,
    }
}

/// Collapses equal neighbours in a sorted slice, returning the kept
/// length. `slice::dedup` is not available without an allocator's
/// `Vec`, and the fixed array is the point.
fn dedup(edges: &mut [f32]) -> usize {
    let mut kept = 0;
    for index in 0..edges.len() {
        let Some(value) = edges.get(index).copied() else {
            break;
        };
        if kept > 0 && edges.get(kept - 1) == Some(&value) {
            continue;
        }
        if let Some(slot) = edges.get_mut(kept) {
            *slot = value;
            kept = kept.wrapping_add(1);
        }
    }
    kept
}

fn holds(region: &Region, (x, y): (f32, f32)) -> bool {
    x >= region.x && x <= region.right() && y >= region.y && y <= region.bottom()
}
