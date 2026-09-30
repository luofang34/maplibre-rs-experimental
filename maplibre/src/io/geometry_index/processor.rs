use std::sync::Arc;

use geo_types::Geometry;
use geozero::{
    error::GeozeroError, geo_types::GeoWriter, ColumnValue, FeatureProcessor, GeomProcessor,
    PropertyProcessor,
};
use rstar::RTree;

use super::{FeatureMeta, IndexedGeometry};
use crate::style::expression::{FeatureProperties, Value};

/// Collects tile-local lines and polygons from a geozero feature stream.
///
/// Multipart and collection geometries yield one entry per line or polygon, in input order,
/// all sharing the feature's typed properties, id and source layer. Points and empty parts are
/// omitted. A feature with no properties receives an empty map. Starting a feature discards any
/// unfinished geometry and properties from a failed feature, retaining completed entries.
/// Geometry-only streams must be enclosed in the feature/geometry callbacks.
pub struct IndexProcessor {
    geo_writer: GeoWriter,
    geometries: Vec<IndexedGeometry<f64>>,
    properties: FeatureProperties,
    coordinate_scale: f64,
    source_layer: Arc<str>,
    ids: Vec<Option<u64>>,
    feature: usize,
}

impl IndexProcessor {
    /// Creates an empty processor with a coordinate scale of one.
    pub fn new() -> Self {
        Self {
            geo_writer: GeoWriter::new(),
            geometries: Vec::new(),
            properties: FeatureProperties::new(),
            coordinate_scale: 1.0,
            source_layer: Arc::from(""),
            ids: Vec::new(),
            feature: 0,
        }
    }

    /// Names the source layer of the features that follow and gives their ids by position, as
    /// the layer's own feature list has them; geozero does not pass ids through.
    pub fn begin_layer(&mut self, source_layer: &str, ids: Vec<Option<u64>>) {
        self.source_layer = Arc::from(source_layer);
        self.ids = ids;
    }

    /// Sets the factor from the next layer's coordinate extent to the 4096 tile grid.
    ///
    /// The factor applies to subsequent coordinates; completed entries are unchanged.
    /// Coordinate callbacks return an error if scaling produces a non-finite value.
    pub fn set_coordinate_scale(&mut self, scale: f64) {
        self.coordinate_scale = scale;
    }

    /// Consumes completed entries into a tree for tile-local spatial queries.
    pub fn build_tree(self) -> RTree<IndexedGeometry<f64>> {
        RTree::bulk_load(self.geometries)
    }

    /// Consumes completed entries in feature and part order, excluding unfinished geometry.
    pub fn get_geometries(self) -> Vec<IndexedGeometry<f64>> {
        self.geometries
    }

    fn index_geometry(&mut self, geometry: Geometry<f64>) {
        let meta = FeatureMeta {
            properties: Arc::new(std::mem::take(&mut self.properties)),
            source_layer: self.source_layer.clone(),
            id: self.ids.get(self.feature).copied().flatten(),
        };
        let mut pending = vec![geometry];
        while let Some(geometry) = pending.pop() {
            let indexed = match geometry {
                Geometry::Polygon(polygon) => IndexedGeometry::from_polygon(polygon, meta.clone()),
                Geometry::LineString(line) => IndexedGeometry::from_linestring(line, meta.clone()),
                Geometry::MultiLineString(lines) => {
                    pending.extend(lines.0.into_iter().rev().map(Geometry::LineString));
                    None
                }
                Geometry::MultiPolygon(polygons) => {
                    pending.extend(polygons.0.into_iter().rev().map(Geometry::Polygon));
                    None
                }
                Geometry::GeometryCollection(collection) => {
                    pending.extend(collection.0.into_iter().rev());
                    None
                }
                _ => None,
            };
            if let Some(indexed) = indexed {
                self.geometries.push(indexed);
            }
        }
    }
}

impl Default for IndexProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl GeomProcessor for IndexProcessor {
    fn xy(&mut self, x: f64, y: f64, idx: usize) -> Result<(), GeozeroError> {
        let scaled_x = x * self.coordinate_scale;
        let scaled_y = y * self.coordinate_scale;
        if !scaled_x.is_finite() || !scaled_y.is_finite() {
            return Err(GeozeroError::Geometry(format!(
                "coordinate {idx} ({x}, {y}) is not finite at scale {}",
                self.coordinate_scale
            )));
        }
        self.geo_writer.xy(scaled_x, scaled_y, idx)
    }
    fn point_begin(&mut self, idx: usize) -> Result<(), GeozeroError> {
        self.geo_writer.point_begin(idx)
    }
    fn point_end(&mut self, idx: usize) -> Result<(), GeozeroError> {
        self.geo_writer.point_end(idx)
    }
    fn multipoint_begin(&mut self, size: usize, idx: usize) -> Result<(), GeozeroError> {
        self.geo_writer.multipoint_begin(size, idx)
    }
    fn multipoint_end(&mut self, idx: usize) -> Result<(), GeozeroError> {
        // Without this the writer keeps the points and the next geometry starts inside them.
        self.geo_writer.multipoint_end(idx)
    }
    fn linestring_begin(
        &mut self,
        tagged: bool,
        size: usize,
        idx: usize,
    ) -> Result<(), GeozeroError> {
        self.geo_writer.linestring_begin(tagged, size, idx)
    }
    fn linestring_end(&mut self, tagged: bool, idx: usize) -> Result<(), GeozeroError> {
        self.geo_writer.linestring_end(tagged, idx)
    }
    fn multilinestring_begin(&mut self, size: usize, idx: usize) -> Result<(), GeozeroError> {
        self.geo_writer.multilinestring_begin(size, idx)
    }
    fn multilinestring_end(&mut self, idx: usize) -> Result<(), GeozeroError> {
        self.geo_writer.multilinestring_end(idx)
    }
    fn polygon_begin(&mut self, tagged: bool, size: usize, idx: usize) -> Result<(), GeozeroError> {
        self.geo_writer.polygon_begin(tagged, size, idx)
    }
    fn polygon_end(&mut self, tagged: bool, idx: usize) -> Result<(), GeozeroError> {
        self.geo_writer.polygon_end(tagged, idx)
    }
    fn multipolygon_begin(&mut self, size: usize, idx: usize) -> Result<(), GeozeroError> {
        self.geo_writer.multipolygon_begin(size, idx)
    }
    fn multipolygon_end(&mut self, idx: usize) -> Result<(), GeozeroError> {
        self.geo_writer.multipolygon_end(idx)
    }
    fn geometrycollection_begin(&mut self, size: usize, idx: usize) -> Result<(), GeozeroError> {
        self.geo_writer.geometrycollection_begin(size, idx)
    }
    fn geometrycollection_end(&mut self, idx: usize) -> Result<(), GeozeroError> {
        self.geo_writer.geometrycollection_end(idx)
    }
}

impl PropertyProcessor for IndexProcessor {
    fn property(
        &mut self,
        _idx: usize,
        name: &str,
        value: &ColumnValue,
    ) -> Result<bool, GeozeroError> {
        self.properties
            .insert(name.to_string(), column_value(value));
        Ok(true)
    }
}

impl FeatureProcessor for IndexProcessor {
    fn feature_begin(&mut self, idx: u64) -> Result<(), GeozeroError> {
        self.geo_writer = GeoWriter::new();
        self.properties.clear();
        self.feature = usize::try_from(idx).unwrap_or(usize::MAX);
        Ok(())
    }

    fn properties_begin(&mut self) -> Result<(), GeozeroError> {
        self.properties.clear();
        Ok(())
    }

    fn geometry_end(&mut self) -> Result<(), GeozeroError> {
        if let Some(geometry) = self.geo_writer.take_geometry() {
            self.index_geometry(geometry);
        }
        Ok(())
    }
}

/// The typed value of a decoded property, as filters compare them.
fn column_value(value: &ColumnValue) -> Value {
    match value {
        ColumnValue::Bool(flag) => Value::Bool(*flag),
        ColumnValue::Byte(number) => Value::Number(f64::from(*number)),
        ColumnValue::UByte(number) => Value::Number(f64::from(*number)),
        ColumnValue::Short(number) => Value::Number(f64::from(*number)),
        ColumnValue::UShort(number) => Value::Number(f64::from(*number)),
        ColumnValue::Int(number) => Value::Number(f64::from(*number)),
        ColumnValue::UInt(number) => Value::Number(f64::from(*number)),
        ColumnValue::Long(number) => Value::Number(*number as f64),
        ColumnValue::ULong(number) => Value::Number(*number as f64),
        ColumnValue::Float(number) => Value::Number(f64::from(*number)),
        ColumnValue::Double(number) => Value::Number(*number),
        other => Value::String(other.to_string()),
    }
}
