//! GeoJSON source processing — projects geographic coordinates into tile space and
//! tessellates features using the existing vector rendering pipeline.

#![deny(missing_docs)]

use std::{borrow::Cow, f64::consts::PI};

use geozero::{FeatureProcessor, GeomProcessor, PropertyProcessor};
use thiserror::Error;

use crate::{
    coords::{WorldTileCoords, EXTENT},
    io::apc::{Context, SendError},
    projection::ProjectionType,
    style::layer::StyleLayer,
    vector::transferables::VectorTransferables,
};

mod geometry_property;
mod process;
mod tessellator;
mod value_reader;

pub(crate) use crate::io::source_client::geojson::store;
pub use crate::io::source_client::geojson::{filter_geojson, index};

/// Failure reported while preparing or delivering GeoJSON tile geometry.
#[derive(Error, Debug)]
pub enum ProcessGeoJsonError {
    /// A worker result could not be delivered through the supplied reply context.
    #[error("sending data back through context failed")]
    SendError(SendError),
    /// Parser detail for invalid GeoJSON input.
    #[error("GeoJSON parsing failed: {0}")]
    Parse(Cow<'static, str>),
}

/// Reprojects longitude/latitude in degrees into Web Mercator tile coordinates for a processor.
/// One tile spans 4096 units. Latitude is clamped to the Mercator range; geometry outside
/// the requested tile is not clipped. Non-finite input coordinates are rejected.
pub struct ProjectingTessellator<T> {
    inner: T,
    tile_x: i32,
    tile_y: i32,
    zoom: u8,
}

impl<T> ProjectingTessellator<T> {
    /// Takes ownership of a processor and fixes the tile origin and zoom used for projection.
    pub fn new(coords: WorldTileCoords, inner: T) -> Self {
        Self {
            inner,
            tile_x: coords.x,
            tile_y: coords.y,
            zoom: u8::from(coords.z),
        }
    }

    /// Convert geographic lon/lat to tile-local extent coordinates (0–4096).
    fn project(&self, lon: f64, lat: f64) -> (f64, f64) {
        let lat = lat.clamp(-85.05112877980659, 85.05112877980659);
        let scale = (1u64 << self.zoom) as f64;
        let mx = (180.0 + lon) / 360.0;
        let my = (180.0 - (180.0 / PI * ((PI / 4.0 + lat * PI / 360.0).tan()).ln())) / 360.0;
        let x = (mx * scale - self.tile_x as f64) * EXTENT;
        let y = (my * scale - self.tile_y as f64) * EXTENT;
        (x, y)
    }

    /// Returns the wrapped processor with any geometry accumulated through this adapter.
    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: GeomProcessor> GeomProcessor for ProjectingTessellator<T> {
    fn xy(&mut self, x: f64, y: f64, idx: usize) -> geozero::error::Result<()> {
        if !x.is_finite() || !y.is_finite() {
            return Err(geozero::error::GeozeroError::Geometry(format!(
                "non-finite GeoJSON coordinate ({x}, {y}) at index {idx}"
            )));
        }
        let (tx, ty) = self.project(x, y);
        if !tx.is_finite() || !ty.is_finite() {
            return Err(geozero::error::GeozeroError::Geometry(format!(
                "GeoJSON coordinate ({x}, {y}) projected to ({tx}, {ty})"
            )));
        }
        self.inner.xy(tx, ty, idx)
    }

    fn point_begin(&mut self, idx: usize) -> geozero::error::Result<()> {
        self.inner.point_begin(idx)
    }

    fn point_end(&mut self, idx: usize) -> geozero::error::Result<()> {
        self.inner.point_end(idx)
    }

    fn multipoint_begin(&mut self, size: usize, idx: usize) -> geozero::error::Result<()> {
        self.inner.multipoint_begin(size, idx)
    }

    fn multipoint_end(&mut self, idx: usize) -> geozero::error::Result<()> {
        self.inner.multipoint_end(idx)
    }

    fn linestring_begin(
        &mut self,
        tagged: bool,
        size: usize,
        idx: usize,
    ) -> geozero::error::Result<()> {
        self.inner.linestring_begin(tagged, size, idx)
    }

    fn linestring_end(&mut self, tagged: bool, idx: usize) -> geozero::error::Result<()> {
        self.inner.linestring_end(tagged, idx)
    }

    fn multilinestring_begin(&mut self, size: usize, idx: usize) -> geozero::error::Result<()> {
        self.inner.multilinestring_begin(size, idx)
    }

    fn multilinestring_end(&mut self, idx: usize) -> geozero::error::Result<()> {
        self.inner.multilinestring_end(idx)
    }

    fn polygon_begin(
        &mut self,
        tagged: bool,
        size: usize,
        idx: usize,
    ) -> geozero::error::Result<()> {
        self.inner.polygon_begin(tagged, size, idx)
    }

    fn polygon_end(&mut self, tagged: bool, idx: usize) -> geozero::error::Result<()> {
        self.inner.polygon_end(tagged, idx)
    }

    fn multipolygon_begin(&mut self, size: usize, idx: usize) -> geozero::error::Result<()> {
        self.inner.multipolygon_begin(size, idx)
    }

    fn multipolygon_end(&mut self, idx: usize) -> geozero::error::Result<()> {
        self.inner.multipolygon_end(idx)
    }

    fn geometrycollection_begin(&mut self, size: usize, idx: usize) -> geozero::error::Result<()> {
        self.inner.geometrycollection_begin(size, idx)
    }

    fn geometrycollection_end(&mut self, idx: usize) -> geozero::error::Result<()> {
        self.inner.geometrycollection_end(idx)
    }
}

impl<T: PropertyProcessor> PropertyProcessor for ProjectingTessellator<T> {
    fn property(
        &mut self,
        idx: usize,
        name: &str,
        value: &geozero::ColumnValue,
    ) -> geozero::error::Result<bool> {
        self.inner.property(idx, name, value)
    }
}

impl<T: FeatureProcessor> FeatureProcessor for ProjectingTessellator<T> {
    fn dataset_begin(&mut self, name: Option<&str>) -> geozero::error::Result<()> {
        self.inner.dataset_begin(name)
    }
    fn dataset_end(&mut self) -> geozero::error::Result<()> {
        self.inner.dataset_end()
    }
    fn feature_begin(&mut self, idx: u64) -> geozero::error::Result<()> {
        self.inner.feature_begin(idx)
    }
    fn properties_begin(&mut self) -> geozero::error::Result<()> {
        self.inner.properties_begin()
    }
    fn properties_end(&mut self) -> geozero::error::Result<()> {
        self.inner.properties_end()
    }
    fn geometry_begin(&mut self) -> geozero::error::Result<()> {
        self.inner.geometry_begin()
    }
    fn geometry_end(&mut self) -> geozero::error::Result<()> {
        self.inner.geometry_end()
    }
    fn feature_end(&mut self, idx: u64) -> geozero::error::Result<()> {
        self.inner.feature_end(idx)
    }
}

/// Request for processing GeoJSON features for a set of style layers.
pub struct GeoJsonTileRequest {
    /// Target tile origin and zoom for projected geometry and expression evaluation.
    pub coords: WorldTileCoords,
    /// Candidate layers, processed only when their source name matches this request.
    pub layers: Vec<StyleLayer>,
    /// Name of the GeoJSON source (used to match style layers by `source` field).
    pub source_name: String,
    /// Projection controlling tessellation density and antimeridian policy.
    pub projection: ProjectionType,
    /// Glyphs and sprites for the layers' symbols; the bundled Latin fallback when absent.
    pub atlas: Option<std::sync::Arc<crate::sdf::assets::SymbolAtlas>>,
    /// The zoom the tile is drawn at when that is past the source's last zoom, so its geometry
    /// is magnified; zero when the tile is drawn at its own zoom.
    pub overscaled_zoom: u8,
    /// Pixels of a 512-pixel tile that the geometry extends past the tile's edge, when the source
    /// sets it; without it the polygons of an extrusion are not clipped to the tile.
    pub buffer: Option<u32>,
    /// Whether the GeoJSON source assigns sequential feature ids (`generateId: true`).
    pub generate_id: bool,
    /// Optional feature property promoted to feature id (`promoteId`).
    pub promote_id: Option<String>,
}

impl GeoJsonTileRequest {
    /// The zoom style expressions are evaluated at.
    fn style_zoom(&self) -> f64 {
        f64::from(self.overscaled_zoom.max(u8::from(self.coords.z)))
    }

    /// How many times the tile is magnified past its source's last zoom.
    fn overscaling(&self) -> f64 {
        let own = u8::from(self.coords.z);
        2_f64.powi(i32::from(self.overscaled_zoom.saturating_sub(own)))
    }
}

/// Process inline GeoJSON data and tessellate features for each matching style layer.
///
/// This mirrors [`crate::vector::process_vector_tile`] but works with geographic
/// (lon/lat) coordinates rather than pre-projected MVT tile coordinates.
///
/// Matching layers with supported paint are filtered and tessellated independently.
/// Results use the style-layer ID as their source-layer key. A filter or geometry failure
/// emits a missing-layer result and processing continues; a delivery failure is returned.
/// The final tile-completion message is sent after all candidate layers have been processed.
pub fn process_geojson_features<T: VectorTransferables, C: Context>(
    geojson_value: &serde_json::Value,
    request: GeoJsonTileRequest,
    context: &C,
) -> Result<(), ProcessGeoJsonError> {
    process::process_geojson_features::<T, C>(geojson_value, request, context)
}

pub mod query;
mod query_index;
pub mod update;

#[cfg(test)]
mod tests;
