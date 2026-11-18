//! Manual serde and `&Value` parser for [`Source`] without `#[serde(tag = "type")]` buffering.

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use super::{
    fresh_generation, CanvasSource, DemEncoding, GeoJsonData, GeoJsonSource, ImageSource,
    PromoteId, RasterDemSource, Source, TileAddressingScheme, VectorSource, VideoSource,
};
use crate::util::json::{
    extract::{
        field_err, json_bool_or, json_f64_or, json_opt_bounds4, json_opt_f64, json_opt_str,
        json_opt_string, json_opt_string_vec, json_opt_u32, json_opt_u64, json_opt_u8,
        json_opt_usize, json_req_coords4, json_req_string, json_string_vec_or_empty,
        missing_field_err,
    },
    serialize_tagged_object,
};

fn parse_scheme(
    map: &Map<String, Value>,
) -> Result<Option<TileAddressingScheme>, serde_json::Error> {
    match json_opt_str(map, "scheme")? {
        None => Ok(None),
        Some("xyz") => Ok(Some(TileAddressingScheme::XYZ)),
        Some("tms") => Ok(Some(TileAddressingScheme::TMS)),
        Some(_) => Err(field_err("scheme", "expected \"xyz\" or \"tms\"")),
    }
}

fn parse_encoding(map: &Map<String, Value>) -> Result<Option<DemEncoding>, serde_json::Error> {
    match json_opt_str(map, "encoding")? {
        None => Ok(None),
        Some("mapbox") => Ok(Some(DemEncoding::Mapbox)),
        Some("terrarium") => Ok(Some(DemEncoding::Terrarium)),
        Some("custom") => Ok(Some(DemEncoding::Custom)),
        Some(_) => Err(field_err("encoding", "unknown DEM encoding")),
    }
}

fn parse_vector_source(map: &Map<String, Value>) -> Result<VectorSource, serde_json::Error> {
    let promote_id = map
        .get("promoteId")
        .filter(|v| !v.is_null())
        .map(PromoteId::from_json_value)
        .transpose()?;
    Ok(VectorSource {
        attribution: json_opt_string(map, "attribution")?,
        bounds: json_opt_bounds4(map, "bounds")?,
        maxzoom: json_opt_u8(map, "maxzoom")?,
        minzoom: json_opt_u8(map, "minzoom")?,
        promote_id,
        scheme: parse_scheme(map)?,
        tiles: json_opt_string_vec(map, "tiles")?,
        tile_size: json_opt_u32(map, "tileSize")?,
        url: json_opt_string(map, "url")?,
        volatile: json_bool_or(map, "volatile", false)?,
    })
}

fn parse_raster_dem_source(map: &Map<String, Value>) -> Result<RasterDemSource, serde_json::Error> {
    Ok(RasterDemSource {
        attribution: json_opt_string(map, "attribution")?,
        bounds: json_opt_bounds4(map, "bounds")?,
        maxzoom: json_opt_u8(map, "maxzoom")?,
        minzoom: json_opt_u8(map, "minzoom")?,
        tiles: json_opt_string_vec(map, "tiles")?,
        url: json_opt_string(map, "url")?,
        tile_size: json_opt_u32(map, "tileSize")?,
        encoding: parse_encoding(map)?,
        red_factor: json_f64_or(map, "redFactor", 1.0)?,
        green_factor: json_f64_or(map, "greenFactor", 1.0)?,
        blue_factor: json_f64_or(map, "blueFactor", 1.0)?,
        base_shift: json_f64_or(map, "baseShift", 0.0)?,
        volatile: json_bool_or(map, "volatile", false)?,
    })
}

fn parse_geojson_source(map: &Map<String, Value>) -> Result<GeoJsonSource, serde_json::Error> {
    let Some(data_val) = map.get("data") else {
        return Err(missing_field_err("data"));
    };
    let promote_id = map
        .get("promoteId")
        .filter(|v| !v.is_null())
        .map(PromoteId::from_json_value)
        .transpose()?;
    Ok(GeoJsonSource {
        data: GeoJsonData::from_json_value(data_val),
        maxzoom: json_opt_u8(map, "maxzoom")?,
        buffer: json_opt_u32(map, "buffer")?,
        tolerance: json_opt_f64(map, "tolerance")?,
        line_metrics: json_bool_or(map, "lineMetrics", false)?,
        minzoom: json_opt_u8(map, "minzoom")?,
        promote_id,
        generate_id: json_bool_or(map, "generateId", false)?,
        cluster: json_bool_or(map, "cluster", false)?,
        cluster_radius: json_opt_f64(map, "clusterRadius")?,
        cluster_max_zoom: json_opt_u8(map, "clusterMaxZoom")?,
        cluster_min_points: json_opt_usize(map, "clusterMinPoints")?,
        cluster_properties: map
            .get("clusterProperties")
            .filter(|v| !v.is_null())
            .cloned(),
        filter: map.get("filter").filter(|v| !v.is_null()).cloned(),
        volatile: json_bool_or(map, "volatile", false)?,
        generation: json_opt_u64(map, "generation")?.unwrap_or_else(fresh_generation),
    })
}

impl Source {
    /// Parses a [`Source`] directly from a JSON [`Value`].
    pub(crate) fn from_json_value(value: &Value) -> Result<Self, serde_json::Error> {
        let Some(map) = value.as_object() else {
            return Err(de::Error::custom("source must be a JSON object"));
        };
        let Some(kind) = map.get("type").and_then(Value::as_str) else {
            return Err(de::Error::missing_field("type"));
        };
        match kind {
            "vector" => Ok(Self::Vector(parse_vector_source(map)?)),
            "raster" => Ok(Self::Raster(parse_vector_source(map)?)),
            "raster-dem" => Ok(Self::RasterDem(parse_raster_dem_source(map)?)),
            "geojson" => Ok(Self::GeoJson(parse_geojson_source(map)?)),
            "image" => Ok(Self::Image(ImageSource {
                url: json_req_string(map, "url")?,
                coordinates: json_req_coords4(map, "coordinates")?,
                generation: json_opt_u64(map, "generation")?.unwrap_or_else(fresh_generation),
            })),
            "video" => Ok(Self::Video(VideoSource {
                urls: json_string_vec_or_empty(map, "urls")?,
                coordinates: json_req_coords4(map, "coordinates")?,
                frame: None,
                generation: fresh_generation(),
            })),
            "canvas" => Ok(Self::Canvas(CanvasSource {
                canvas: json_opt_string(map, "canvas")?,
                coordinates: json_req_coords4(map, "coordinates")?,
                animate: json_bool_or(map, "animate", true)?,
                frame: None,
                generation: fresh_generation(),
            })),
            other => Err(de::Error::unknown_variant(
                other,
                &[
                    "vector",
                    "raster",
                    "raster-dem",
                    "geojson",
                    "image",
                    "video",
                    "canvas",
                ],
            )),
        }
    }
}

impl<'de> Deserialize<'de> for Source {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Self::from_json_value(&value).map_err(de::Error::custom)
    }
}

impl Serialize for Source {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Vector(source) => serialize_tagged_object("vector", source, serializer),
            Self::Raster(source) => serialize_tagged_object("raster", source, serializer),
            Self::RasterDem(source) => serialize_tagged_object("raster-dem", source, serializer),
            Self::GeoJson(source) => serialize_tagged_object("geojson", source, serializer),
            Self::Image(source) => serialize_tagged_object("image", source, serializer),
            Self::Video(source) => serialize_tagged_object("video", source, serializer),
            Self::Canvas(source) => serialize_tagged_object("canvas", source, serializer),
        }
    }
}
