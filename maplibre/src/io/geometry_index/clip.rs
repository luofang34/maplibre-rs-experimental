//! Cutting an indexed geometry down to a tile and its buffer, as GL JS's `geojson-vt` cuts a
//! GeoJSON feature for each tile it reaches: one axis at a time, each ring on its own.
//!
//! A feature indexed whole in every tile it reaches would be found once for each of them, and
//! an extrusion's roof would stand in a neighbour's index where that tile holds none of it.

use geo_types::{Coord, LineString, Polygon, Rect};

use super::{ExactGeometry, FeatureMeta, IndexedGeometry};

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

impl IndexedGeometry<f64> {
    /// The geometry in the units of a child tile `scale` times finer whose top left corner lies
    /// at `offset` of the child's units, as GL JS moves an overzoomed tile's features into it.
    pub fn rescaled(&self, scale: f64, offset: [f64; 2]) -> Self {
        use geo::MapCoords;
        let map = |coord: geo_types::Coord<f64>| geo_types::Coord {
            x: coord.x * scale - offset[0],
            y: coord.y * scale - offset[1],
        };
        let exact = match &self.exact {
            ExactGeometry::Polygon(polygon) => ExactGeometry::Polygon(polygon.map_coords(map)),
            ExactGeometry::LineString(line) => ExactGeometry::LineString(line.map_coords(map)),
            ExactGeometry::Point(point) => ExactGeometry::Point(point.map_coords(map)),
        };
        let (lower, upper) = (self.bounds.lower(), self.bounds.upper());
        let (lower, upper) = (map(lower.0), map(upper.0));
        Self {
            bounds: rstar::AABB::from_corners(lower.into(), upper.into()),
            exact,
            properties: self.properties.clone(),
            source_layer: self.source_layer.clone(),
            id: self.id,
            feature_index: self.feature_index,
        }
    }

    /// The parts of the geometry within `area`, each indexed on its own.
    pub fn clipped_to(self, area: Rect<f64>) -> Vec<Self> {
        let meta = FeatureMeta {
            properties: self.properties,
            source_layer: self.source_layer,
            id: self.id,
            feature_index: self.feature_index,
        };
        let slabs = slabs(area);
        match self.exact {
            ExactGeometry::Polygon(polygon) => {
                let cut = |ring: &LineString<f64>| {
                    slabs
                        .iter()
                        .fold(ring.0.clone(), |ring, slab| slab.ring(&ring))
                };
                let exterior = cut(polygon.exterior());
                if exterior.len() < 4 {
                    return Vec::new();
                }
                let holes = polygon
                    .interiors()
                    .iter()
                    .map(cut)
                    .filter(|ring| ring.len() >= 4)
                    .map(LineString::new)
                    .collect();
                Self::from_polygon(Polygon::new(LineString::new(exterior), holes), meta)
                    .into_iter()
                    .collect()
            }
            ExactGeometry::Point(point) => {
                let (min, max) = (area.min(), area.max());
                let inside =
                    (min.x..=max.x).contains(&point.x()) && (min.y..=max.y).contains(&point.y());
                inside
                    .then(|| Self::from_point(point, meta))
                    .flatten()
                    .into_iter()
                    .collect()
            }
            ExactGeometry::LineString(line) => slabs
                .iter()
                .fold(vec![line.0], |runs, slab| {
                    runs.iter().flat_map(|run| slab.line(run)).collect()
                })
                .into_iter()
                .filter_map(|run| Self::from_linestring(LineString::new(run), meta.clone()))
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests;
