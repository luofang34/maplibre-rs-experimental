//! Tile-local geometry and property storage for point queries.

#![deny(missing_docs)]

mod processor;
pub use processor::IndexProcessor;

use std::collections::{BTreeMap, HashMap};

use cgmath::{num_traits::Signed, Bounded};
use geo::prelude::*;
use geo_types::{Coord, CoordFloat, LineString, Point, Polygon};
use rstar::{Envelope, PointDistance, RTree, RTreeObject, AABB};

use crate::{
    coords::{
        InnerCoords, Quadkey, WorldCoords, WorldTileCoords, Zoom, ZoomLevel, EXTENT, TILE_SIZE,
    },
    util::math::bounds_from_points,
};

/// Query indexes keyed by canonical tile quadkeys; each tile has one replaceable entry.
pub struct GeometryIndex {
    index: BTreeMap<Quadkey, TileIndex>,
}

impl GeometryIndex {
    /// Creates an index with no tiles.
    pub fn new() -> Self {
        Self {
            index: Default::default(),
        }
    }

    /// Replaces the index at `coords`; unaddressable tile coordinates are ignored.
    pub fn index_tile(&mut self, coords: &WorldTileCoords, tile_index: TileIndex) {
        coords
            .build_quad_key()
            .and_then(|key| self.index.insert(key, tile_index));
    }

    /// Forgets a tile's index when the tile leaves the store.
    pub fn remove(&mut self, coords: &WorldTileCoords) {
        if let Some(key) = coords.build_quad_key() {
            self.index.remove(&key);
        }
    }

    /// Memory the index holds, estimated from its geometries and properties.
    pub fn approximate_bytes(&self) -> usize {
        self.index.values().map(TileIndex::approximate_bytes).sum()
    }

    /// Estimated memory retained by one tile's geometries and query properties.
    pub(crate) fn tile_bytes(&self, coords: WorldTileCoords) -> usize {
        coords
            .build_quad_key()
            .and_then(|key| self.index.get(&key))
            .map_or(0, TileIndex::approximate_bytes)
    }

    /// Finds geometries at world pixels measured at `zoom`, in the tile at grid level `z`.
    ///
    /// Returns `None` if that tile has no index; an indexed tile with no hits yields `Some`
    /// of an empty vector. Does not search loaded ancestors, wrap coordinates, filter style
    /// layers, or sort by draw order. See [`TileIndex::point_query`] for hit tolerance.
    pub fn query_point(
        &self,
        world_coords: &WorldCoords,
        z: ZoomLevel,
        zoom: Zoom,
    ) -> Option<Vec<&IndexedGeometry<f64>>> {
        let world_tile_coords = world_coords.into_world_tile(z, zoom);

        if let Some(index) = world_tile_coords
            .build_quad_key()
            .and_then(|key| self.index.get(&key))
        {
            let scale = zoom.scale_to_zoom_level(z);

            let delta_x = world_coords.x / TILE_SIZE * scale - world_tile_coords.x as f64;
            let delta_y = world_coords.y / TILE_SIZE * scale - world_tile_coords.y as f64;

            let x = delta_x * EXTENT;
            let y = delta_y * EXTENT;
            Some(index.point_query(InnerCoords { x, y }))
        } else {
            None
        }
    }
}

impl Default for GeometryIndex {
    fn default() -> Self {
        Self::new()
    }
}

/// Geometries for one tile, stored either in input order or in a spatial tree.
pub enum TileIndex {
    /// Spatial storage; query hits are ordered by distance, with unspecified tie order.
    Spatial {
        /// Tree whose envelopes and geometry use the same tile-local units.
        tree: RTree<IndexedGeometry<f64>>,
    },
    /// Sequential storage; query hits retain their position in the list.
    Linear {
        /// Geometries in the order to return matching entries.
        list: Vec<IndexedGeometry<f64>>,
    },
}

impl TileIndex {
    /// Memory the tile's index holds, estimated from its geometries and properties.
    pub fn approximate_bytes(&self) -> usize {
        match self {
            TileIndex::Spatial { tree } => {
                tree.iter().map(IndexedGeometry::approximate_bytes).sum()
            }
            TileIndex::Linear { list } => list.iter().map(IndexedGeometry::approximate_bytes).sum(),
        }
    }

    /// Returns polygon interiors and lines within eight tile-local units of the point.
    ///
    /// Polygon boundaries and holes are excluded. Coordinates use the 4096-unit tile grid;
    /// the line tolerance is not a screen-pixel distance. No style visibility, layer order,
    /// or feature deduplication is applied; multipart features can yield several entries.
    pub fn point_query(&self, inner_coords: InnerCoords) -> Vec<&IndexedGeometry<f64>> {
        let point = Point::new(inner_coords.x, inner_coords.y);
        let coordinate: Coord<_> = point.into();

        match self {
            TileIndex::Spatial { tree } => tree
                .nearest_neighbor_iter(&point)
                .filter(|geometry| match &geometry.exact {
                    ExactGeometry::Polygon(exact) => exact.contains(&coordinate),
                    ExactGeometry::LineString(exact) => exact.distance_2(&point) <= 64.0,
                })
                .collect::<Vec<_>>(),
            TileIndex::Linear { list } => list
                .iter()
                .filter(|geometry| match &geometry.exact {
                    ExactGeometry::Polygon(exact) => exact.contains(&coordinate),
                    ExactGeometry::LineString(exact) => exact.distance_2(&point) <= 64.0,
                })
                .collect::<Vec<_>>(),
        }
    }
}

/// A nonempty tile-local geometry, its enclosing bounds, and stringified feature properties.
///
/// Callers constructing entries directly must provide finite coordinates and bounds enclosing
/// the complete geometry. Spatial distance queries use the geometry, including polygon holes.
#[derive(Debug, Clone)]
pub struct IndexedGeometry<T>
where
    T: CoordFloat + Bounded + Signed,
{
    /// Envelope enclosing `exact`, in the same coordinate units.
    pub bounds: AABB<Point<T>>,
    /// Geometry used for hit tests and spatial distance.
    pub exact: ExactGeometry<T>,
    /// Feature properties; scalar types are represented as strings.
    pub properties: HashMap<String, String>,
}

/// A single polygon or line part in tile-local coordinates.
#[derive(Debug, Clone)]
pub enum ExactGeometry<T>
where
    T: CoordFloat + Bounded + Signed,
{
    /// Polygon exterior and any interior holes.
    Polygon(Polygon<T>),
    /// Ordered vertices of a line.
    LineString(LineString<T>),
}

impl<T> IndexedGeometry<T>
where
    T: CoordFloat + Bounded + Signed + PartialOrd,
{
    /// Memory the geometry holds: its coordinates, its property strings and the map and
    /// tree entries that carry them.
    pub fn approximate_bytes(&self) -> usize {
        const ENTRY_OVERHEAD: usize = 2 * std::mem::size_of::<String>() + 16;
        const TREE_OVERHEAD: usize = 32;
        let coordinates = match &self.exact {
            ExactGeometry::Polygon(polygon) => {
                polygon.exterior().0.len()
                    + polygon
                        .interiors()
                        .iter()
                        .map(|ring| ring.0.len())
                        .sum::<usize>()
            }
            ExactGeometry::LineString(line) => line.0.len(),
        };
        let properties: usize = self
            .properties
            .iter()
            .map(|(key, value)| key.capacity() + value.capacity() + ENTRY_OVERHEAD)
            .sum();
        std::mem::size_of::<Self>()
            + coordinates * std::mem::size_of::<Coord<T>>()
            + properties
            + TREE_OVERHEAD
    }

    fn from_polygon(polygon: Polygon<T>, properties: HashMap<String, String>) -> Option<Self> {
        let (min, max) = bounds_from_points(polygon.exterior().points())?;

        Some(Self {
            exact: ExactGeometry::Polygon(polygon),
            bounds: AABB::from_corners(Point::from(min), Point::from(max)),
            properties,
        })
    }
    fn from_linestring(
        linestring: LineString<T>,
        properties: HashMap<String, String>,
    ) -> Option<Self> {
        let (min, max) = bounds_from_points(linestring.points())?;

        Some(Self {
            exact: ExactGeometry::LineString(linestring),
            bounds: AABB::from_corners(Point::from(min), Point::from(max)),
            properties,
        })
    }
}

impl<T> RTreeObject for IndexedGeometry<T>
where
    T: CoordFloat + Bounded + Signed + PartialOrd,
{
    type Envelope = AABB<Point<T>>;

    fn envelope(&self) -> Self::Envelope {
        self.bounds
    }
}

impl<T> PointDistance for IndexedGeometry<T>
where
    T: geo::GeoFloat,
{
    fn distance_2(
        &self,
        point: &<Self::Envelope as Envelope>::Point,
    ) -> <<Self::Envelope as Envelope>::Point as rstar::Point>::Scalar {
        let distance = match &self.exact {
            ExactGeometry::Polygon(polygon) => polygon.euclidean_distance(point),
            ExactGeometry::LineString(line) => line.euclidean_distance(point),
        };
        distance * distance
    }
}

#[cfg(test)]
mod tests;
