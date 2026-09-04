//! Vector tile data utilities.

use serde::{Deserialize, Serialize};

/// String url to a tile.
pub type TileUrl = String;

/// String url to a JSON tile.
pub type TileJSONUrl = String;

/// Tiles can be positioned using either the xyz coordinates or the TMS (Tile Map Service) protocol.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileAddressingScheme {
    #[serde(rename = "xyz")]
    XYZ,
    #[serde(rename = "tms")]
    TMS,
}

impl Default for TileAddressingScheme {
    fn default() -> Self {
        TileAddressingScheme::XYZ
    }
}

/// GeoJSON data — either an inline JSON value or a URL pointing to a GeoJSON file.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum GeoJsonData {
    Url(String),
    Inline(serde_json::Value),
}

/// Source properties for a GeoJSON source.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GeoJsonSource {
    pub data: GeoJsonData,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maxzoom: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minzoom: Option<u8>,
}

/// Source properties for tiles or rasters.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct VectorSource {
    /// String which contains attribution information for the used tiles.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attribution: Option<String>,
    /// The bounds in which tiles are available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounds: Option<(f64, f64, f64, f64)>,
    /// Max zoom level at which tiles are available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maxzoom: Option<u8>,
    /// Min zoom level at which tiles are available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minzoom: Option<u8>,
    // TODO: promoteId
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
    // TODO volatile
}

/// How elevation is packed into the RGB channels of a `raster-dem` tile.
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
    /// The bounds in which tiles are available.
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
    /// Edge length of one tile in pixels.
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

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum Source {
    #[serde(rename = "vector")]
    Vector(VectorSource),
    #[serde(rename = "raster")]
    Raster(VectorSource), // FIXME: Does it make sense that a raster have a VectorSource?
    #[serde(rename = "raster-dem")]
    RasterDem(RasterDemSource),
    #[serde(rename = "geojson")]
    GeoJson(GeoJsonSource),
}

#[cfg(test)]
mod tests;
