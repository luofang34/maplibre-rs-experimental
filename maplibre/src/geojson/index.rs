//! A GeoJSON document parsed once into a spatial index, tiled into vector tile bytes on demand.
//!
//! Tiles are cut from the index instead of re-reading the document, so a source is parsed once
//! per generation however many tiles are requested. Geometry is not clipped or simplified: the
//! tile boundary is applied by the renderer's stencil.

use geozero::mvt::{Message, Tile};
use rstar::{RTree, RTreeObject, AABB};
use serde_json::Value;
use thiserror::Error;

use crate::{
    coords::WorldTileCoords,
    style::source::{GeoJsonSource, PromoteId, GEOJSON_LAYER},
};

/// Vector tile grid the features are quantized to.
pub const EXTENT: u32 = 4096;
/// Distance beyond the tile edge whose features are included, in tile grid units (GL JS default).
const BUFFER: f64 = 128.0;
/// Mercator latitude limit in degrees.
const MAX_LATITUDE: f64 = 85.051_128_78;

/// Why a document could not be indexed.
#[derive(Debug, Error)]
pub enum GeoJsonError {
    /// The text is not JSON.
    #[error("GeoJSON is not valid JSON")]
    Json(#[source] serde_json::Error),
    /// The document is neither a feature collection, a feature nor a geometry.
    #[error("GeoJSON root has unsupported type `{found}`")]
    Root {
        /// The `type` member found, or an empty string.
        found: String,
    },
    /// A feature's geometry cannot be read.
    #[error("GeoJSON feature {index} has an invalid geometry: {reason}")]
    Geometry {
        /// Position of the feature in the document.
        index: usize,
        /// What is wrong with it.
        reason: &'static str,
    },
}

type Point = [f64; 2];
type Ring = Vec<Point>;

/// Points, lines and polygons of one feature in world coordinates, where the world is `0..1`.
#[derive(Default, Clone)]
struct Geometry {
    points: Vec<Point>,
    lines: Vec<Ring>,
    polygons: Vec<Vec<Ring>>,
}

#[derive(Clone)]
struct IndexedFeature {
    id: Option<u64>,
    geometry: Geometry,
    properties: Vec<(String, Value)>,
}

struct Entry {
    bounds: [f64; 4],
    feature: u32,
}

impl RTreeObject for Entry {
    type Envelope = AABB<Point>;

    fn envelope(&self) -> Self::Envelope {
        AABB::from_corners(
            [self.bounds[0], self.bounds[1]],
            [self.bounds[2], self.bounds[3]],
        )
    }
}

/// Features of one GeoJSON document, findable by the tile they fall in.
pub struct GeoJsonIndex {
    features: Vec<IndexedFeature>,
    /// The points of a clustering source, and how they group at each zoom.
    clustered: Option<(Vec<IndexedFeature>, Clusters)>,
    /// Whether the source filters its features before they are tiled.
    filtered: bool,
    tree: RTree<Entry>,
    approximate_bytes: usize,
}

impl GeoJsonIndex {
    /// Indexes the document text a source URL returned.
    pub fn from_text(text: &[u8], source: &GeoJsonSource) -> Result<Self, GeoJsonError> {
        let value = serde_json::from_slice(text).map_err(GeoJsonError::Json)?;
        Self::from_value(&value, source)
    }

    /// Indexes an inline document, applying the source's id options.
    pub fn from_value(document: &Value, source: &GeoJsonSource) -> Result<Self, GeoJsonError> {
        let promote = promoted_property(source.promote_id.as_ref());
        let mut features = Vec::new();
        let mut entries = Vec::new();
        let mut bytes = 0;
        let mut position = 0_usize;
        let filter = source_filter(source);
        let filter_declared = filter.is_some();
        let mut points = Vec::new();
        let mut add = |feature: &Value| -> Result<(), GeoJsonError> {
            let index = position;
            position += 1;
            if filter
                .as_ref()
                .is_some_and(|filter| !feature_passes(feature, filter, 0.0))
            {
                return Ok(());
            }
            let Some(geometry) = feature.get("geometry").filter(|value| !value.is_null()) else {
                return Ok(());
            };
            let mut parsed = Geometry::default();
            read_geometry(geometry, &mut parsed)
                .map_err(|reason| GeoJsonError::Geometry { index, reason })?;
            let Some(bounds) = bounds_of(&parsed) else {
                return Ok(());
            };
            let properties: Vec<(String, Value)> = feature
                .get("properties")
                .and_then(Value::as_object)
                .map(|map| map.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default();
            let id = numeric_feature_id(feature, promote, source.generate_id.then_some(index));
            if source.cluster && is_single_point(geometry, &parsed) {
                points.push(IndexedFeature {
                    id,
                    geometry: parsed,
                    properties,
                });
                return Ok(());
            }
            bytes += coordinate_count(&parsed) * 16
                + properties
                    .iter()
                    .map(|(key, value)| key.len() + value.to_string().len())
                    .sum::<usize>()
                + 64;
            entries.push(Entry {
                bounds,
                feature: features.len() as u32,
            });
            features.push(IndexedFeature {
                id,
                geometry: parsed,
                properties,
            });
            Ok(())
        };
        match document.get("type").and_then(Value::as_str) {
            Some("FeatureCollection") => {
                for feature in document
                    .get("features")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    add(feature)?;
                }
            }
            Some("Feature") => add(document)?,
            Some(
                "Point" | "MultiPoint" | "LineString" | "MultiLineString" | "Polygon"
                | "MultiPolygon" | "GeometryCollection",
            ) => add(&serde_json::json!({ "geometry": document }))?,
            other => {
                return Err(GeoJsonError::Root {
                    found: other.unwrap_or_default().to_owned(),
                })
            }
        }
        let clustered = source.cluster.then(|| {
            let inputs: Vec<ClusterInput<'_>> = points
                .iter()
                .filter_map(|feature| {
                    Some((
                        *feature.geometry.points.first()?,
                        feature.properties.as_slice(),
                    ))
                })
                .collect();
            let options = ClusterOptions::new(
                source.cluster_radius,
                source.cluster_max_zoom,
                source.cluster_min_points,
                source.cluster_properties.as_ref(),
                source.maxzoom,
            );
            let clusters = Clusters::new(&inputs, options);
            (points, clusters)
        });
        Ok(Self {
            features,
            clustered,
            filtered: filter_declared,
            tree: RTree::bulk_load(entries),
            approximate_bytes: bytes,
        })
    }

    /// Number of indexed features.
    pub fn len(&self) -> usize {
        self.features.len()
    }

    /// Whether the document held no drawable feature.
    pub fn is_empty(&self) -> bool {
        self.features.is_empty()
    }

    /// Memory the index holds, for the shared cache's byte budget.
    pub fn approximate_bytes(&self) -> usize {
        self.approximate_bytes
    }

    /// The GeoJSON a source with a filter or clusters presents to `coords`: the features of
    /// `document` that pass the filter, with its points replaced by the clusters they form at
    /// that zoom. `None` for a source that presents its document unchanged.
    pub fn source_document(
        &self,
        document: &Value,
        source: &GeoJsonSource,
        coords: WorldTileCoords,
    ) -> Option<Value> {
        let clusters = self.clustered.as_ref().map(|(_, clusters)| clusters);
        if clusters.is_none() && !self.filtered {
            return None;
        }
        let filter = source_filter(source);
        let members: Vec<&Value> = match document.get("type").and_then(Value::as_str) {
            Some("FeatureCollection") => document
                .get("features")
                .and_then(Value::as_array)
                .map(|features| features.iter().collect())
                .unwrap_or_default(),
            _ => vec![document],
        };
        let mut features: Vec<Value> = members
            .into_iter()
            .filter(|feature| {
                filter
                    .as_ref()
                    .is_none_or(|filter| feature_passes(feature, filter, 0.0))
            })
            .filter(|feature| {
                clusters.is_none()
                    || !feature.get("geometry").is_some_and(|geometry| {
                        geometry.get("type").and_then(Value::as_str) == Some("Point")
                    })
            })
            .cloned()
            .collect();
        if let Some(clusters) = clusters {
            let zoom = u32::from(u8::from(coords.z));
            let tiles = f64::from(1_u32 << zoom.min(30));
            let (west, north) = (
                f64::from(coords.x.rem_euclid(1_i32 << zoom.min(30))) / tiles,
                f64::from(coords.y) / tiles,
            );
            let margin = BUFFER / f64::from(EXTENT) / tiles;
            let window = [
                west - margin,
                north - margin,
                west + 1.0 / tiles + margin,
                north + 1.0 / tiles + margin,
            ];
            let points = &self.clustered.as_ref()?.0;
            for item in clusters.within(zoom.min(24) as u8, window) {
                let (position, id, properties) = match item {
                    Clustered::Point(index, position) => {
                        (position, points[index].id, points[index].properties.clone())
                    }
                    Clustered::Cluster(position, id, properties) => {
                        (position, Some(id), properties)
                    }
                };
                let mut feature = serde_json::json!({
                    "type": "Feature",
                    "properties": properties.into_iter().collect::<serde_json::Map<String, Value>>(),
                    "geometry": {"type": "Point", "coordinates": super::cluster::unproject(position)},
                });
                if let Some(id) = id {
                    feature["id"] = serde_json::json!(id);
                }
                features.push(feature);
            }
        }
        Some(serde_json::json!({"type": "FeatureCollection", "features": features}))
    }

    /// Vector tile bytes with the features that touch `coords`, in document order.
    pub fn tile(&self, coords: WorldTileCoords) -> Vec<u8> {
        let zoom = u32::from(u8::from(coords.z));
        let tiles = f64::from(1_u32 << zoom.min(30));
        let column = f64::from(coords.x.rem_euclid(1_i32 << zoom.min(30)));
        let row = f64::from(coords.y);
        let margin = BUFFER / f64::from(EXTENT) / tiles;
        let (west, north) = (column / tiles, row / tiles);
        let window = AABB::from_corners(
            [west - margin, north - margin],
            [west + 1.0 / tiles + margin, north + 1.0 / tiles + margin],
        );
        let mut hits: Vec<u32> = self
            .tree
            .locate_in_envelope_intersecting(&window)
            .map(|entry| entry.feature)
            .collect();
        hits.sort_unstable();
        let mut encoder = TileEncoder::default();
        let to_tile = |point: &Point| -> (i32, i32) {
            (
                ((point[0] - west) * tiles * f64::from(EXTENT)).round() as i32,
                ((point[1] - north) * tiles * f64::from(EXTENT)).round() as i32,
            )
        };
        for index in hits {
            let feature = &self.features[index as usize];
            encoder.feature(feature, &to_tile);
        }
        if let Some((points, clusters)) = &self.clustered {
            let window = [
                window.lower()[0],
                window.lower()[1],
                window.upper()[0],
                window.upper()[1],
            ];
            for item in clusters.within(zoom.min(24) as u8, window) {
                let feature = match item {
                    Clustered::Point(index, _) => points[index].clone(),
                    Clustered::Cluster(position, id, properties) => IndexedFeature {
                        id: Some(id),
                        geometry: Geometry {
                            points: vec![position],
                            ..Geometry::default()
                        },
                        properties,
                    },
                };
                encoder.feature(&feature, &to_tile);
            }
        }
        Tile {
            layers: vec![encoder.finish()],
        }
        .encode_to_vec()
    }
}

fn promoted_property(promote: Option<&PromoteId>) -> Option<&str> {
    match promote? {
        PromoteId::Property(name) => Some(name),
        PromoteId::PerLayer(map) => map.get(GEOJSON_LAYER).map(String::as_str),
    }
}

/// The id a feature carries into tiles and queries: the promoted property or `id` when it is a
/// whole non-negative number, else its position in the document with `generateId`.
pub(super) fn numeric_feature_id(
    feature: &Value,
    promoted: Option<&str>,
    generated: Option<usize>,
) -> Option<u64> {
    promoted
        .and_then(|name| feature.get("properties")?.get(name))
        .and_then(integer_id)
        .or_else(|| feature.get("id").and_then(integer_id))
        .or(generated.map(|index| index as u64))
}

fn integer_id(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        let number = value.as_f64()?;
        (number >= 0.0 && number.fract() == 0.0 && number < 9.0e15).then_some(number as u64)
    })
}

fn project(position: &Value) -> Result<Point, &'static str> {
    let coordinates = position.as_array().ok_or("position is not an array")?;
    let longitude = coordinates
        .first()
        .and_then(Value::as_f64)
        .ok_or("position has no longitude")?;
    let latitude = coordinates
        .get(1)
        .and_then(Value::as_f64)
        .ok_or("position has no latitude")?;
    let latitude = latitude.clamp(-MAX_LATITUDE, MAX_LATITUDE).to_radians();
    Ok([
        (longitude + 180.0) / 360.0,
        0.5 - (std::f64::consts::FRAC_PI_4 + latitude / 2.0).tan().ln()
            / (2.0 * std::f64::consts::PI),
    ])
}

fn read_ring(value: &Value) -> Result<Ring, &'static str> {
    value
        .as_array()
        .ok_or("coordinates are not an array")?
        .iter()
        .map(project)
        .collect()
}

fn read_rings(value: &Value) -> Result<Vec<Ring>, &'static str> {
    value
        .as_array()
        .ok_or("coordinates are not an array")?
        .iter()
        .map(read_ring)
        .collect()
}

fn read_geometry(geometry: &Value, out: &mut Geometry) -> Result<(), &'static str> {
    let coordinates = geometry.get("coordinates");
    let need = || coordinates.ok_or("geometry has no coordinates");
    match geometry.get("type").and_then(Value::as_str) {
        Some("Point") => out.points.push(project(need()?)?),
        Some("MultiPoint") => out.points.extend(read_ring(need()?)?),
        Some("LineString") => out.lines.push(read_ring(need()?)?),
        Some("MultiLineString") => out.lines.extend(read_rings(need()?)?),
        Some("Polygon") => out.polygons.push(read_rings(need()?)?),
        Some("MultiPolygon") => {
            for polygon in need()?.as_array().ok_or("coordinates are not an array")? {
                out.polygons.push(read_rings(polygon)?);
            }
        }
        Some("GeometryCollection") => {
            for member in geometry
                .get("geometries")
                .and_then(Value::as_array)
                .ok_or("collection has no geometries")?
            {
                read_geometry(member, out)?;
            }
        }
        _ => return Err("unsupported geometry type"),
    }
    Ok(())
}

fn positions(geometry: &Geometry) -> impl Iterator<Item = &Point> {
    geometry
        .points
        .iter()
        .chain(geometry.lines.iter().flatten())
        .chain(geometry.polygons.iter().flatten().flatten())
}

fn coordinate_count(geometry: &Geometry) -> usize {
    positions(geometry).count()
}

fn bounds_of(geometry: &Geometry) -> Option<[f64; 4]> {
    positions(geometry).fold(None, |bounds, point| {
        let [min_x, min_y, max_x, max_y] =
            bounds.unwrap_or([point[0], point[1], point[0], point[1]]);
        Some([
            min_x.min(point[0]),
            min_y.min(point[1]),
            max_x.max(point[0]),
            max_y.max(point[1]),
        ])
    })
}

mod encode;
use encode::TileEncoder;

use super::{
    cluster::{ClusterInput, ClusterOptions, Clustered, Clusters},
    feature_passes,
};

/// The source's own filter, when it has a valid one.
fn source_filter(source: &GeoJsonSource) -> Option<crate::style::filter::Filter> {
    crate::style::filter::Filter::parse(source.filter.as_ref()?).ok()
}

/// Whether a feature is one point, the only geometry that clusters.
fn is_single_point(geometry: &Value, parsed: &Geometry) -> bool {
    geometry.get("type").and_then(Value::as_str) == Some("Point")
        && parsed.points.len() == 1
        && parsed.lines.is_empty()
        && parsed.polygons.is_empty()
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
