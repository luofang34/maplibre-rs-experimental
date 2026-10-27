//! Cutting rings and lines down to a rectangle, as GL JS's `geojson-vt` cuts a GeoJSON feature
//! for each tile it reaches: one axis at a time, each ring on its own.
//!
//! A cut ring follows the rectangle's edges where it left it, so a ring that encloses the whole
//! rectangle becomes the rectangle itself.

use geo_types::{Coord, Rect};

/// One side of the clipping rectangle: the axis it bounds and the range kept on it.
#[derive(Clone, Copy)]
struct Slab {
    axis: usize,
    min: f64,
    max: f64,
}

impl Slab {
    fn of(coord: Coord<f64>, axis: usize) -> f64 {
        if axis == 0 {
            coord.x
        } else {
            coord.y
        }
    }

    /// Where the segment `a`–`b` crosses `at` on this slab's axis.
    fn crossing(&self, a: Coord<f64>, b: Coord<f64>, at: f64) -> Coord<f64> {
        let (from, to) = (Self::of(a, self.axis), Self::of(b, self.axis));
        let t = (at - from) / (to - from);
        let point = Coord {
            x: a.x + (b.x - a.x) * t,
            y: a.y + (b.y - a.y) * t,
        };
        // The crossing lies exactly on the edge, whatever rounding the interpolation does.
        if self.axis == 0 {
            Coord { x: at, ..point }
        } else {
            Coord { y: at, ..point }
        }
    }

    /// A closed ring cut to the slab, with the slab's edges closing what it cut off.
    fn ring(&self, ring: &[Coord<f64>]) -> Vec<Coord<f64>> {
        let mut kept = Vec::with_capacity(ring.len());
        for pair in ring.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let (from, to) = (Self::of(a, self.axis), Self::of(b, self.axis));
            let inside = |value: f64| (self.min..=self.max).contains(&value);
            if inside(from) {
                kept.push(a);
            }
            // A segment can leave across one edge and come back across the other.
            let mut crossings = [self.min, self.max]
                .into_iter()
                .filter(|edge| (from - edge) * (to - edge) < 0.0)
                .collect::<Vec<_>>();
            if from > to {
                crossings.reverse();
            }
            kept.extend(crossings.into_iter().map(|edge| self.crossing(a, b, edge)));
        }
        if let Some(&first) = kept.first() {
            kept.push(first);
        }
        kept
    }

    /// The runs of a line within the slab.
    fn line(&self, line: &[Coord<f64>]) -> Vec<Vec<Coord<f64>>> {
        let inside =
            |coord: Coord<f64>| (self.min..=self.max).contains(&Self::of(coord, self.axis));
        let mut runs = Vec::new();
        let mut run: Vec<Coord<f64>> = Vec::new();
        for (index, &point) in line.iter().enumerate() {
            if index > 0 {
                let a = line[index - 1];
                let (from, to) = (Self::of(a, self.axis), Self::of(point, self.axis));
                let mut crossings = [self.min, self.max]
                    .into_iter()
                    .filter(|edge| (from - edge) * (to - edge) < 0.0)
                    .collect::<Vec<_>>();
                if from > to {
                    crossings.reverse();
                }
                for edge in crossings {
                    let crossing = self.crossing(a, point, edge);
                    if run.is_empty() {
                        run.push(crossing);
                    } else {
                        run.push(crossing);
                        runs.push(std::mem::take(&mut run));
                    }
                }
            }
            if inside(point) {
                run.push(point);
            }
        }
        runs.push(run);
        runs.retain(|run| run.len() >= 2);
        runs
    }
}

fn slabs(area: Rect<f64>) -> [Slab; 2] {
    let (min, max) = (area.min(), area.max());
    [
        Slab {
            axis: 0,
            min: min.x,
            max: max.x,
        },
        Slab {
            axis: 1,
            min: min.y,
            max: max.y,
        },
    ]
}

/// The closed ring `ring` cut to `area`, closed again; fewer than four points when nothing of
/// it encloses any of `area`.
pub(crate) fn ring_within(ring: &[Coord<f64>], area: Rect<f64>) -> Vec<Coord<f64>> {
    let [across, down] = slabs(area);
    down.ring(&across.ring(ring))
}

/// The runs of `line` within `area`, each at least two points long.
pub(crate) fn line_within(line: Vec<Coord<f64>>, area: Rect<f64>) -> Vec<Vec<Coord<f64>>> {
    slabs(area).iter().fold(vec![line], |runs, slab| {
        runs.iter().flat_map(|run| slab.line(run)).collect()
    })
}

#[cfg(test)]
mod tests;
