//! Named vector, image, elevation and GeoJSON source definitions.

use std::{collections::HashMap, sync::Arc};

use serde::{Deserialize, Serialize};

/// Tile URL template, which may contain `{z}`, `{x}` and `{y}` placeholders.
pub type TileUrl = String;

/// URL of a TileJSON metadata document, distinct from an individual tile URL.
pub type TileJSONUrl = String;

/// Tiles can be positioned using either the xyz coordinates or the TMS (Tile Map Service) protocol.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TileAddressingScheme {
    /// Rows start at the north edge of the world.
    #[serde(rename = "xyz")]
    #[default]
    XYZ,
    /// Rows start at the south edge; the tile loader flips the Y coordinate.
    #[serde(rename = "tms")]
    TMS,
}

/// GeoJSON data — either an inline JSON value or a URL pointing to a GeoJSON file.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum GeoJsonData {
    /// Address of a GeoJSON document.
    Url(String),
    /// Embedded feature, feature collection or geometry JSON, shared by every request.
    Inline(Arc<serde_json::Value>),
}

/// Feature property that replaces the feature id, as the style specification's `promoteId`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(untagged)]
pub enum PromoteId {
    /// One property for every layer of the source.
    Property(String),
    /// A property for each source layer; GeoJSON has the single layer `_geojson`.
    PerLayer(HashMap<String, String>),
}

/// The vector tile layer name GeoJSON features are tiled into, as GL JS names it.
pub const GEOJSON_LAYER: &str = "_geojson";

/// GeoJSON source declaration; workers fetch URL data, index it once per generation and tile it.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GeoJsonSource {
    /// Embedded geometry or a document URL.
    pub data: GeoJsonData,
    /// Upper zoom the source is tiled at; deeper views overzoom it. GL JS defaults to 18.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maxzoom: Option<u8>,
    /// Lower source zoom; nothing is tiled below it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minzoom: Option<u8>,
    /// Property whose value becomes the feature id.
    #[serde(rename = "promoteId", default, skip_serializing_if = "Option::is_none")]
    pub promote_id: Option<PromoteId>,
    /// Numbers features without an id by their position in the document.
    #[serde(rename = "generateId", default, skip_serializing_if = "is_false")]
    pub generate_id: bool,
    /// Identifies the data version; a changed value makes workers load and index it again.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub generation: u64,
}

fn is_false(value: &bool) -> bool {
    !value
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

/// The GL JS default for a GeoJSON source's `maxzoom`.
pub const GEOJSON_DEFAULT_MAXZOOM: u8 = 18;

/// TileJSON-compatible addressing shared by vector and raster image sources.
/// Raster sources additionally use `tile_size` to choose their visible tile zoom.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct VectorSource {
    /// String which contains attribution information for the used tiles.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attribution: Option<String>,
    /// Declared availability bounds as `(west, south, east, north)` in geographic degrees.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounds: Option<(f64, f64, f64, f64)>,
    /// Max zoom level at which tiles are available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maxzoom: Option<u8>,
    /// Min zoom level at which tiles are available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minzoom: Option<u8>,
    /// Y-axis convention for tile URL expansion; omission uses XYZ.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheme: Option<TileAddressingScheme>,
    /// Array of URLs which can contain place holders like {x}, {y}, {z}.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tiles: Option<Vec<TileUrl>>,
    /// Edge length in pixels of one tile; raster sources are often 256, the default is 512.
    #[serde(rename = "tileSize", skip_serializing_if = "Option::is_none")]
    pub tile_size: Option<u32>,
    /// URL of a TileJSON document that supplies the tile URLs and zoom range.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<TileJSONUrl>,
}

/// How elevation in meters is packed into 8-bit RGB channels of a `raster-dem` tile.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DemEncoding {
    /// Mapbox Terrain-RGB: `(R * 256 * 256 + G * 256 + B) / 10 - 10000`.
    #[default]
    #[serde(rename = "mapbox")]
    Mapbox,
    /// Terrarium: `R * 256 + G + B / 256 - 32768`.
    #[serde(rename = "terrarium")]
    Terrarium,
    /// Channel factors and base shift taken from the source definition.
    #[serde(rename = "custom")]
    Custom,
}

/// Source properties for elevation tiles.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct RasterDemSource {
    /// String which contains attribution information for the used tiles.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attribution: Option<String>,
    /// Declared availability bounds as `(west, south, east, north)` in geographic degrees.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounds: Option<(f64, f64, f64, f64)>,
    /// Max zoom level at which tiles are available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maxzoom: Option<u8>,
    /// Min zoom level at which tiles are available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minzoom: Option<u8>,
    /// Array of URLs which can contain place holders like {x}, {y}, {z}.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tiles: Option<Vec<TileUrl>>,
    /// URL of a TileJSON document that supplies the tile URLs and zoom range.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<TileJSONUrl>,
    /// Nominal tile size in pixels used for zoom selection; omission defaults to 512.
    #[serde(rename = "tileSize", default = "default_dem_tile_size")]
    pub tile_size: u32,
    /// Elevation packing of the tile pixels.
    #[serde(default)]
    pub encoding: DemEncoding,
    /// Red channel factor for the `custom` encoding.
    #[serde(rename = "redFactor", default = "default_channel_factor")]
    pub red_factor: f64,
    /// Green channel factor for the `custom` encoding.
    #[serde(rename = "greenFactor", default = "default_channel_factor")]
    pub green_factor: f64,
    /// Blue channel factor for the `custom` encoding.
    #[serde(rename = "blueFactor", default = "default_channel_factor")]
    pub blue_factor: f64,
    /// Value subtracted after channel weighting for the `custom` encoding.
    #[serde(rename = "baseShift", default)]
    pub base_shift: f64,
}

fn default_dem_tile_size() -> u32 {
    512
}

fn default_channel_factor() -> f64 {
    1.0
}

impl RasterDemSource {
    /// Returns `[red, green, blue, base_shift]` such that
    /// `elevation = R * red + G * green + B * blue - base_shift` for 0..=255 channel values.
    pub fn unpack_vector(&self) -> [f64; 4] {
        match self.encoding {
            DemEncoding::Mapbox => [6553.6, 25.6, 0.1, 10000.0],
            DemEncoding::Terrarium => [256.0, 1.0, 1.0 / 256.0, 32768.0],
            DemEncoding::Custom => [
                self.red_factor,
                self.green_factor,
                self.blue_factor,
                self.base_shift,
            ],
        }
    }
}

/// Data source selected by the JSON `type` field.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum Source {
    /// Vector tile geometry and feature properties.
    #[serde(rename = "vector")]
    Vector(VectorSource),
    /// Raster image tiles, using the same URL and TileJSON fields as vector sources.
    #[serde(rename = "raster")]
    Raster(VectorSource),
    /// Packed elevation images consumed by terrain, hillshade and relief rendering.
    #[serde(rename = "raster-dem")]
    RasterDem(RasterDemSource),
    /// GeoJSON features declared inline or by document URL.
    #[serde(rename = "geojson")]
    GeoJson(GeoJsonSource),
}

#[cfg(test)]
mod tests;
