//! Whether a fill or line feature meets a query on the ground plane, as GL JS
//! `FillStyleLayer` and `LineStyleLayer.queryIntersectsFeature` decide it.
//!
//! The query is the ground the screen point or box covers, which a pitched view turns into a
//! trapezoid; a translated layer moves it back by its translation, and a line is met within half
//! its width of it once it is moved sideways by its offset.

use geo::prelude::*;
use geo_types::{Coord, LineString, Point, Polygon};

use crate::{
    io::geometry_index::{ExactGeometry, IndexedGeometry},
    style::{expression::FeatureProperties, layer::LinePaint},
};

/// The ground a query covers, in the units of one tile.
#[derive(Clone, Debug)]
pub(super) enum Footprint {
    /// The ground under a screen point.
    Point(Point<f64>),
    /// The ground under a screen box, a trapezoid when the view is pitched.
    Polygon(Polygon<f64>),
}

impl Footprint {
    /// The footprint of ground corners: one for a point, four for a box.
    pub(super) fn of(corners: &[[f64; 2]]) -> Option<Self> {
        match corners {
            [] => None,
            [point] => Some(Self::Point(Point::new(point[0], point[1]))),
            _ if corners.iter().all(|corner| corner == &corners[0]) => {
                Some(Self::Point(Point::new(corners[0][0], corners[0][1])))
            }
            _ => Some(Self::Polygon(Polygon::new(
                LineString::from(corners.iter().map(|&[x, y]| (x, y)).collect::<Vec<_>>()),
                Vec::new(),
            ))),
        }
    }

    /// The footprint moved by `[dx, dy]`.
    fn shifted(&self, [dx, dy]: [f64; 2]) -> Self {
        let shift = |coord: Coord<f64>| Coord {
            x: coord.x + dx,
            y: coord.y + dy,
        };
        match self {
            Self::Point(point) => Self::Point(Point::from(shift(point.0))),
            Self::Polygon(polygon) => Self::Polygon(polygon.map_coords(shift)),
        }
    }

    fn meets(&self, polygon: &Polygon<f64>) -> bool {
        match self {
            Self::Point(point) => polygon.intersects(point),
            Self::Polygon(footprint) => polygon.intersects(footprint),
        }
    }

    fn distance(&self, line: &LineString<f64>) -> f64 {
        match self {
            Self::Point(point) => point.euclidean_distance(line),
            Self::Polygon(footprint) => footprint.euclidean_distance(line),
        }
    }
}

/// Half a line's drawn width and its sideways offset, in screen pixels, as GL JS
/// `getLineWidth` and `line-offset` give them for one feature.
pub(super) fn line_reach(paint: &LinePaint, properties: &FeatureProperties, zoom: f64) -> [f64; 2] {
    let value = |property: &Option<crate::style::property::StyleProperty<f32>>, default: f32| {
        property
            .as_ref()
            .and_then(|value| value.evaluate_for(properties, zoom))
            .map_or(f64::from(default), f64::from)
    };
    let width = value(&paint.line_width, 1.0);
    let gap = value(&paint.line_gap_width, 0.0);
    let drawn = if gap > 0.0 { gap + 2.0 * width } else { width };
    [drawn / 2.0, value(&paint.line_offset, 0.0)]
}

/// A line moved `offset` units to its right, each vertex along the bisector of its corner, as
/// GL JS `offsetLine`.
fn offset_line(line: &LineString<f64>, offset: f64) -> LineString<f64> {
    let mut points: Vec<Coord<f64>> = line.coords().copied().collect();
    points.dedup();
    let normal = |from: Coord<f64>, to: Coord<f64>| {
        let (dx, dy) = (to.x - from.x, to.y - from.y);
        let length = dx.hypot(dy);
        // GL JS's perpendicular of (x, y) is (-y, x).
        [-dy / length, dx / length]
    };
    let shifted = (0..points.len())
        .map(|index| {
            let before = index.checked_sub(1).map_or([0.0, 0.0], |previous| {
                normal(points[previous], points[index])
            });
            let after = points
                .get(index + 1)
                .map_or([0.0, 0.0], |&next| normal(points[index], next));
            let sum = [before[0] + after[0], before[1] + after[1]];
            let length = sum[0].hypot(sum[1]);
            let mut bisector = if length > 0.0 {
                [sum[0] / length, sum[1] / length]
            } else {
                [0.0, 0.0]
            };
            let cos_half_angle = bisector[0] * after[0] + bisector[1] * after[1];
            if cos_half_angle != 0.0 {
                bisector = [bisector[0] / cos_half_angle, bisector[1] / cos_half_angle];
            }
            Coord {
                x: points[index].x + bisector[0] * offset,
                y: points[index].y + bisector[1] * offset,
            }
        })
        .collect();
    LineString::new(shifted)
}

/// Whether `geometry`, drawn by a layer of `kind` moved by `translate` tile units, meets the
/// query's `footprint`; `line` is a line layer's half width and offset in tile units.
pub(super) fn touches(
    geometry: &IndexedGeometry<f64>,
    footprint: &Footprint,
    (kind, translate): (&str, [f64; 2]),
    line: [f64; 2],
) -> bool {
    // A translated layer draws a feature away from where it is, so the query is moved back.
    let query = footprint.shifted([-translate[0], -translate[1]]);
    let [half_width, offset] = line;
    let near = |ring: &LineString<f64>| {
        let ring = if offset == 0.0 {
            ring.clone()
        } else {
            offset_line(ring, offset)
        };
        query.distance(&ring) <= half_width
    };
    match (&geometry.exact, kind) {
        (ExactGeometry::Polygon(polygon), "fill") => query.meets(polygon),
        (ExactGeometry::Polygon(polygon), "line") => {
            near(polygon.exterior()) || polygon.interiors().iter().any(near)
        }
        (ExactGeometry::LineString(line), "line") => near(line),
        _ => false,
    }
}

#[cfg(test)]
mod tests;
