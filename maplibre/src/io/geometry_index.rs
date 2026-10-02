//! Tile-local geometry and property storage for point queries.

#![deny(missing_docs)]

mod clip;
mod processor;
use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

use cgmath::{num_traits::Signed, Bounded};
use geo::prelude::*;
use geo_types::{Coord, CoordFloat, LineString, Point, Polygon};
pub use processor::IndexProcessor;
use rstar::{Envelope, PointDistance, RTree, RTreeObject, AABB};

use crate::{
    coords::{
        InnerCoords, Quadkey, WorldCoords, WorldTileCoords, Zoom, ZoomLevel, EXTENT, TILE_SIZE,
    },
    style::expression::FeatureProperties,
    util::math::bounds_from_points,
};

/// Query indexes keyed by canonical tile quadkeys; each tile has one replaceable entry for every
/// source that contributed to it.
pub struct GeometryIndex {
    index: BTreeMap<Quadkey, BTreeMap<Option<String>, TileIndex>>,
}

impl GeometryIndex {
    /// Creates an index with no tiles.
    pub fn new() -> Self {
        Self {
            index: Default::default(),
        }
    }

    /// Replaces the index a source holds at `coords`, leaving other sources' entries in place;
    /// unaddressable tile coordinates are ignored. A source is the style's name for it, or
    /// `None` for layers that name no source.
    pub fn index_tile(
        &mut self,
        coords: &WorldTileCoords,
        source: Option<String>,
        tile_index: TileIndex,
    ) {
        if let Some(key) = coords.build_quad_key() {
            self.index
                .entry(key)
                .or_default()
                .insert(source, tile_index);
        }
    }

    /// The indexes of every source at `coords`, or `None` when the tile has none.
    pub fn tile_indexes(
        &self,
        coords: &WorldTileCoords,
    ) -> Option<impl Iterator<Item = (Option<&str>, &TileIndex)>> {
        let sources = self.index.get(&coords.build_quad_key()?)?;
        Some(
            sources
                .iter()
                .map(|(source, index)| (source.as_deref(), index)),
        )
    }

    /// Forgets a tile's index when the tile leaves the store.
    pub fn remove(&mut self, coords: &WorldTileCoords) {
        if let Some(key) = coords.build_quad_key() {
            self.index.remove(&key);
        }
    }

    /// Memory the index holds, estimated from its geometries and properties.
    pub fn approximate_bytes(&self) -> usize {
        self.index
            .values()
            .flat_map(BTreeMap::values)
            .map(TileIndex::approximate_bytes)
            .sum()
    }

    /// Estimated memory retained by one tile's geometries and query properties.
    pub(crate) fn tile_bytes(&self, coords: WorldTileCoords) -> usize {
        coords
            .build_quad_key()
            .and_then(|key| self.index.get(&key))
            .map_or(0, |sources| {
                sources.values().map(TileIndex::approximate_bytes).sum()
            })
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

        if let Some(sources) = world_tile_coords
            .build_quad_key()
            .and_then(|key| self.index.get(&key))
        {
            let scale = zoom.scale_to_zoom_level(z);

            let delta_x = world_coords.x / TILE_SIZE * scale - world_tile_coords.x as f64;
            let delta_y = world_coords.y / TILE_SIZE * scale - world_tile_coords.y as f64;

            let x = delta_x * EXTENT;
            let y = delta_y * EXTENT;
            Some(
                sources
                    .values()
                    .flat_map(|index| index.point_query(InnerCoords { x, y }))
                    .collect(),
            )
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
        let mut shared = HashSet::new();
        let mut total = 0;
        let mut count = |geometry: &IndexedGeometry<f64>| {
            total += geometry.geometry_bytes();
            if shared.insert(Arc::as_ptr(&geometry.properties)) {
                total += properties_bytes(&geometry.properties);
            }
        };
        match self {
            TileIndex::Spatial { tree } => tree.iter().for_each(&mut count),
            TileIndex::Linear { list } => list.iter().for_each(&mut count),
        }
        total
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
                    ExactGeometry::Point(_) => false,
                })
                .collect::<Vec<_>>(),
            TileIndex::Linear { list } => list
                .iter()
                .filter(|geometry| match &geometry.exact {
                    ExactGeometry::Polygon(exact) => exact.contains(&coordinate),
                    ExactGeometry::LineString(exact) => exact.distance_2(&point) <= 64.0,
                    ExactGeometry::Point(_) => false,
                })
                .collect::<Vec<_>>(),
        }
    }
}

/// What identifies the feature a geometry part belongs to: shared by all its parts.
#[derive(Debug, Clone, Default)]
pub struct FeatureMeta {
    /// Typed feature properties, shared by every part of the feature.
    pub properties: Arc<FeatureProperties>,
    /// The source layer the feature was read from.
    pub source_layer: Arc<str>,
    /// The feature's id, when the source assigned one.
    pub id: Option<u64>,
    /// The feature's place in its source layer, which orders query results top-down as GL JS
    /// orders them by feature index.
    pub feature_index: u32,
}

/// A nonempty tile-local geometry, its enclosing bounds and the feature it belongs to.
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
    /// Typed feature properties, shared by every part of the feature.
    pub properties: Arc<FeatureProperties>,
    /// The source layer the feature was read from.
    pub source_layer: Arc<str>,
    /// The feature's id, when the source assigned one.
    pub id: Option<u64>,
    /// The feature's place in its source layer, which orders query results top-down as GL JS
    /// orders them by feature index.
    pub feature_index: u32,
}

/// A single polygon, line or point part in tile-local coordinates.
#[derive(Debug, Clone)]
pub enum ExactGeometry<T>
where
    T: CoordFloat + Bounded + Signed,
{
    /// Polygon exterior and any interior holes.
    Polygon(Polygon<T>),
    /// Ordered vertices of a line.
    LineString(LineString<T>),
    /// One point, as each point of a multi-point is its own part.
    Point(Point<T>),
}

impl<T> IndexedGeometry<T>
where
    T: CoordFloat + Bounded + Signed + PartialOrd,
{
    /// Memory the geometry holds: its coordinates, and its properties, which parts of one
    /// feature share; [`TileIndex::approximate_bytes`] counts those once.
    pub fn approximate_bytes(&self) -> usize {
        self.geometry_bytes() + properties_bytes(&self.properties)
    }

    /// Memory of the geometry alone.
    fn geometry_bytes(&self) -> usize {
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
            ExactGeometry::Point(_) => 1,
        };
        std::mem::size_of::<Self>() + coordinates * std::mem::size_of::<Coord<T>>() + TREE_OVERHEAD
    }

    fn from_polygon(polygon: Polygon<T>, meta: FeatureMeta) -> Option<Self> {
        let (min, max) = bounds_from_points(polygon.exterior().points())?;

        Some(Self {
            exact: ExactGeometry::Polygon(polygon),
            bounds: AABB::from_corners(Point::from(min), Point::from(max)),
            properties: meta.properties,
            source_layer: meta.source_layer,
            id: meta.id,
            feature_index: meta.feature_index,
        })
    }
    fn from_point(point: Point<T>, meta: FeatureMeta) -> Option<Self> {
        Some(Self {
            exact: ExactGeometry::Point(point),
            bounds: AABB::from_corners(point, point),
            properties: meta.properties,
            source_layer: meta.source_layer,
            id: meta.id,
            feature_index: meta.feature_index,
        })
    }

    fn from_linestring(linestring: LineString<T>, meta: FeatureMeta) -> Option<Self> {
        let (min, max) = bounds_from_points(linestring.points())?;

        Some(Self {
            exact: ExactGeometry::LineString(linestring),
            bounds: AABB::from_corners(Point::from(min), Point::from(max)),
            properties: meta.properties,
            source_layer: meta.source_layer,
            id: meta.id,
            feature_index: meta.feature_index,
        })
    }
}

/// Memory of a feature's properties: keys, values and the map entries that carry them.
fn properties_bytes(properties: &FeatureProperties) -> usize {
    const ENTRY_OVERHEAD: usize = 2 * std::mem::size_of::<String>() + 16;
    properties
        .iter()
        .map(|(key, value)| {
            key.capacity()
                + match value {
                    crate::style::expression::Value::String(text) => text.capacity(),
                    _ => 0,
                }
                + ENTRY_OVERHEAD
        })
        .sum()
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
            ExactGeometry::Point(at) => at.euclidean_distance(point),
        };
        distance * distance
    }
}

#[cfg(test)]
mod tests;
