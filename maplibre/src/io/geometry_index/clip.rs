//! Cutting an indexed geometry down to a tile and its buffer, as GL JS's `geojson-vt` cuts a
//! GeoJSON feature for each tile it reaches: one axis at a time, each ring on its own.
//!
//! A feature indexed whole in every tile it reaches would be found once for each of them, and
//! an extrusion's roof would stand in a neighbour's index where that tile holds none of it.

use geo_types::{LineString, Polygon, Rect};

use super::{ExactGeometry, FeatureMeta, IndexedGeometry};
use crate::coords::clip::{line_within, ring_within};

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
        match self.exact {
            ExactGeometry::Polygon(polygon) => {
                let cut = |ring: &LineString<f64>| ring_within(&ring.0, area);
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
            ExactGeometry::LineString(line) => line_within(line.0, area)
                .into_iter()
                .filter_map(|run| Self::from_linestring(LineString::new(run), meta.clone()))
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests;
