//! GeoJSON source processing — projects geographic coordinates into tile space and
//! tessellates features using the existing vector rendering pipeline.

#![deny(missing_docs)]

use std::{borrow::Cow, f64::consts::PI};

use geozero::{FeatureProcessor, GeomProcessor, GeozeroDatasource, PropertyProcessor};
use thiserror::Error;

use crate::{
    coords::{WorldTileCoords, EXTENT},
    io::apc::{Context, SendError},
    projection::{globe::subdivision::granularity_for_zoom, ProjectionType},
    sdf::tessellation::TextTessellator,
    style::{
        expression::Value,
        filter::{properties_from_json, FeatureContext, Filter, GeometryType},
        layer::{LayerPaint, StyleLayer},
    },
    vector::{
        tessellation::{CircleOptions, ExtrusionOptions, IndexDataType, ZeroTessellator},
        transferables::{
            LayerMissing, LayerTessellated, SymbolLayerTessellated, TileTessellated,
            VectorTransferables,
        },
    },
};

mod cluster;

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
}

/// Whether one GeoJSON feature passes a layer filter.
fn feature_passes(feature: &serde_json::Value, filter: &Filter, zoom: f64) -> bool {
    let properties = properties_from_json(feature.get("properties"));
    let geometry_type = feature
        .get("geometry")
        .and_then(|geometry| geometry.get("type"))
        .and_then(serde_json::Value::as_str)
        .map_or(GeometryType::Unknown, GeometryType::from_geojson);
    filter.evaluate(&FeatureContext {
        properties: &properties,
        geometry_type,
        id: feature.get("id").map(Value::from_json),
        zoom,
    })
}

/// The GeoJSON a layer sees after its filter: a collection keeps the passing features, a lone
/// feature or geometry either stays or becomes an empty collection.
pub fn filter_geojson(
    geojson: &serde_json::Value,
    filter: &Filter,
    zoom: f64,
) -> serde_json::Value {
    let empty = serde_json::json!({"type": "FeatureCollection", "features": []});
    match geojson.get("type").and_then(serde_json::Value::as_str) {
        Some("FeatureCollection") => {
            let features: Vec<serde_json::Value> = geojson
                .get("features")
                .and_then(serde_json::Value::as_array)
                .map(|features| {
                    features
                        .iter()
                        .filter(|feature| feature_passes(feature, filter, zoom))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            serde_json::json!({"type": "FeatureCollection", "features": features})
        }
        Some("Feature") if feature_passes(geojson, filter, zoom) => geojson.clone(),
        Some("Feature") => empty,
        Some(_) => {
            let feature = serde_json::json!({"type": "Feature", "geometry": geojson});
            if feature_passes(&feature, filter, zoom) {
                geojson.clone()
            } else {
                empty
            }
        }
        None => geojson.clone(),
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
    let coords = request.coords;
    let unfiltered = geojson_value.to_string();

    for style_layer in &request.layers {
        let matches_source = style_layer
            .source
            .as_deref()
            .is_some_and(|source| source == request.source_name);
        if !matches_source {
            continue;
        }

        let Some(paint) = &style_layer.paint else {
            log::warn!("GeoJSON style layer {} has no paint", style_layer.id);
            continue;
        };

        let json_str = match &style_layer.filter {
            Some(filter) => match Filter::parse(filter) {
                Ok(filter) => filter_geojson(geojson_value, &filter, f64::from(u8::from(coords.z)))
                    .to_string(),
                Err(error) => {
                    tracing::error!(
                        layer = %style_layer.id,
                        %error,
                        "unsupported filter; the layer renders nothing"
                    );
                    context
                        .send_back(T::LayerMissing::build_from(coords, style_layer.id.clone()))
                        .map_err(ProcessGeoJsonError::SendError)?;
                    continue;
                }
            },
            None => unfiltered.clone(),
        };

        match paint {
            LayerPaint::Fill(_)
            | LayerPaint::FillExtrusion(_)
            | LayerPaint::Line(_)
            | LayerPaint::Background(_)
            | LayerPaint::Circle(_)
            | LayerPaint::Heatmap(_) => {
                let zoom = u8::from(coords.z);
                let granularity = match paint {
                    LayerPaint::Fill(_) => granularity_for_zoom(128, 2, zoom),
                    LayerPaint::Line(_) => granularity_for_zoom(512, 0, zoom),
                    _ => 1,
                };
                let use_globe_geometry = request.projection.uses_globe_rendering(f64::from(zoom));
                let mut tessellator = match paint {
                    LayerPaint::Circle(circle) => ZeroTessellator::<IndexDataType>::default()
                        .with_circles(CircleOptions::for_paint(circle, f64::from(zoom))),
                    LayerPaint::Heatmap(heatmap) => ZeroTessellator::<IndexDataType>::default()
                        .with_circles(CircleOptions::for_heatmap(heatmap, f64::from(zoom))),
                    LayerPaint::FillExtrusion(extrusion) => {
                        ZeroTessellator::<IndexDataType>::default()
                            .with_extrusion(ExtrusionOptions::for_paint(extrusion))
                    }
                    _ if use_globe_geometry => {
                        let last_tile =
                            i64::from(crate::coords::ZOOM_BOUNDS[usize::from(zoom)]) - 1;
                        ZeroTessellator::<IndexDataType>::default().with_globe_subdivision(
                            granularity,
                            zoom == 0,
                            coords.y == 0,
                            i64::from(coords.y) == last_tile,
                        )
                    }
                    _ => ZeroTessellator::<IndexDataType>::default(),
                }
                .with_feature_opacity(paint.opacity(), f64::from(zoom));
                match paint {
                    LayerPaint::Fill(p) if p.fill_pattern.is_some() => {
                        tessellator.fallback_color = [1.0; 4]
                    }
                    LayerPaint::Fill(p) => {
                        tessellator.style_property = p.fill_color.clone();
                        tessellator.outline_property = p.fill_outline_color.clone();
                    }
                    LayerPaint::FillExtrusion(p) if p.fill_extrusion_pattern.is_some() => {
                        tessellator.fallback_color = [1.0; 4]
                    }
                    LayerPaint::FillExtrusion(p) => {
                        tessellator.style_property = p.fill_extrusion_color.clone()
                    }
                    LayerPaint::Circle(p) => tessellator.style_property = p.circle_color.clone(),
                    LayerPaint::Line(p) => {
                        tessellator.style_property = p.line_color.clone();
                        tessellator.is_line_layer = true;
                        tessellator.line_gradient = p.line_gradient.is_some();
                        tessellator.line_feature_style =
                            crate::vector::tessellation::LineFeatureStyle::for_paint(
                                p,
                                f64::from(zoom),
                            );
                        tessellator.stroke =
                            crate::style::line_stroke::LineStroke::of_layer(style_layer);
                    }
                    LayerPaint::Background(p) => {
                        tessellator.style_property = p.background_color.clone()
                    }
                    _ => {}
                }

                let mut projecting = ProjectingTessellator::new(coords, tessellator);

                let mut geojson_src = geozero::geojson::GeoJson(json_str.as_str());
                if let Err(e) = geojson_src.process(&mut projecting) {
                    log::warn!(
                        "GeoJSON tessellation for layer {} failed: {e:?}",
                        style_layer.id
                    );
                    context
                        .send_back(T::LayerMissing::build_from(coords, style_layer.id.clone()))
                        .map_err(ProcessGeoJsonError::SendError)?;
                    continue;
                }

                let mut inner = projecting.into_inner();
                // For bare GeoJSON geometries (Polygon, LineString, etc. — not a
                // FeatureCollection), geozero never calls `feature_end`, so
                // `feature_indices` stays empty while `buffer.indices` is not.
                // Manually commit the remaining geometry as a single feature.
                if inner.feature_indices.is_empty() && !inner.buffer.indices.is_empty() {
                    let _ = inner.feature_end(0);
                }

                let synthetic_layer = geozero::mvt::tile::Layer {
                    version: 2,
                    name: style_layer.id.clone(),
                    ..Default::default()
                };

                context
                    .send_back(T::LayerTessellated::build_from(
                        coords,
                        inner.buffer.into(),
                        inner.feature_indices,
                        inner.feature_colors,
                        synthetic_layer,
                        style_layer.id.clone(),
                    ))
                    .map_err(ProcessGeoJsonError::SendError)?;
            }
            LayerPaint::Symbol(symbol_paint) => {
                let zoom = f64::from(u8::from(coords.z));
                let atlas = request
                    .atlas
                    .clone()
                    .unwrap_or_else(crate::sdf::assets::fallback_atlas);
                let tessellator =
                    TextTessellator::with_assets(symbol_paint.clone(), zoom, atlas.clone());
                let mut projecting = ProjectingTessellator::new(coords, tessellator);

                let mut geojson_src = geozero::geojson::GeoJson(json_str.as_str());
                if let Err(e) = geojson_src.process(&mut projecting) {
                    log::warn!(
                        "GeoJSON text tessellation for layer {} failed: {e:?}",
                        style_layer.id
                    );
                    context
                        .send_back(T::LayerMissing::build_from(coords, style_layer.id.clone()))
                        .map_err(ProcessGeoJsonError::SendError)?;
                    continue;
                }

                let mut inner = projecting.into_inner();
                // A bare geometry never reaches feature_end, so its symbol is committed here.
                if inner.features.is_empty() {
                    let _ = inner.feature_end(0);
                }
                inner.finish();

                let synthetic_layer = geozero::mvt::tile::Layer {
                    version: 2,
                    name: style_layer.id.clone(),
                    ..Default::default()
                };

                context
                    .send_back(T::SymbolLayerTessellated::build_from(
                        coords,
                        inner.quad_buffer.into(),
                        inner.features,
                        Some(atlas),
                        synthetic_layer,
                        style_layer.id.clone(),
                    ))
                    .map_err(ProcessGeoJsonError::SendError)?;
            }
            _ => {
                log::trace!(
                    "GeoJSON layer {} has unsupported paint type, skipping",
                    style_layer.id
                );
            }
        }
    }

    context
        .send_back(T::TileTessellated::build_from(coords))
        .map_err(ProcessGeoJsonError::SendError)?;

    Ok(())
}

pub mod index;
pub mod query;
mod store;
pub mod update;

#[cfg(test)]
mod tests;
