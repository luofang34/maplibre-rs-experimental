//! Fetchable tile sources addressed by URL templates.

use thiserror::Error;

use crate::{
    coords::WorldTileCoords,
    style::source::{GeoJsonSource, ImageSource, TileAddressingScheme},
};

/// Tile coordinates that cannot be addressed by any tile URL.
#[derive(Debug, Error)]
#[error("tile coordinates {coords} are outside the addressable range")]
pub struct InvalidTileCoords {
    /// Coordinates that failed to convert into an addressing scheme.
    pub coords: WorldTileCoords,
}

/// Half the Web Mercator world's width, metres: the EPSG:3857 extent.
const MERCATOR_HALF_EXTENT_M: f64 = 20_037_508.342_789_244;

/// The tile's `minx,miny,maxx,maxy` in EPSG:3857 metres, as MapLibre's
/// `{bbox-epsg-3857}` placeholder states it for WMS-style servers. The box
/// is the tile's ground, so it does not depend on the row scheme.
fn bbox_epsg_3857(coords: &WorldTileCoords) -> Option<String> {
    let tile = coords.into_tile(TileAddressingScheme::XYZ)?;
    let size = 2.0 * MERCATOR_HALF_EXTENT_M / f64::from(1_u32 << u8::from(tile.z));
    let min_x = f64::from(tile.x) * size - MERCATOR_HALF_EXTENT_M;
    let max_y = MERCATOR_HALF_EXTENT_M - f64::from(tile.y) * size;
    Some(format!("{min_x},{},{},{max_y}", max_y - size, min_x + size))
}

fn format_template(
    template: &str,
    scheme: TileAddressingScheme,
    coords: &WorldTileCoords,
) -> Option<String> {
    let tile = coords.into_tile(scheme)?;
    let mut url = template
        .replace("{z}", &u8::from(tile.z).to_string())
        .replace("{x}", &tile.x.to_string())
        .replace("{y}", &tile.y.to_string());
    if url.contains("{bbox-epsg-3857}") {
        url = url.replace("{bbox-epsg-3857}", &bbox_epsg_3857(coords)?);
    }
    Some(url)
}

/// Represents a source from which vector tiles are fetched.
#[derive(Clone, Debug)]
pub struct TessellateSource {
    template: String,
    scheme: TileAddressingScheme,
}

impl TessellateSource {
    /// Creates a source addressed as `{url}/{z}/{x}/{y}.{filetype}`.
    pub fn new(url: &str, filetype: &str) -> Self {
        Self::from_template(
            format!("{url}/{{z}}/{{x}}/{{y}}.{filetype}"),
            TileAddressingScheme::XYZ,
        )
    }

    /// Creates a source from a style tile URL template containing `{z}`, `{x}` and `{y}`, or
    /// `{bbox-epsg-3857}`.
    pub fn from_template(template: impl Into<String>, scheme: TileAddressingScheme) -> Self {
        Self {
            template: template.into(),
            scheme,
        }
    }

    /// Returns the URL template with unexpanded placeholders.
    pub fn template(&self) -> &str {
        &self.template
    }

    /// Returns the tile URL, or `None` when the coordinates are outside the world.
    pub fn format(&self, coords: &WorldTileCoords) -> Option<String> {
        format_template(&self.template, self.scheme, coords)
    }
}

impl Default for TessellateSource {
    fn default() -> Self {
        Self::new("https://maps.tuerantuer.org/europe_germany", "pbf")
    }
}

/// Represents a source from which raster tiles are fetched.
#[derive(Clone, Debug)]
pub struct RasterSource {
    template: String,
    scheme: TileAddressingScheme,
}

impl RasterSource {
    /// Creates a source addressed as `{url}/{z}/{x}/{y}.{filetype}?key={key}`.
    pub fn new(url: &str, filetype: &str, key: &str) -> Self {
        Self::from_template(
            format!("{url}/{{z}}/{{x}}/{{y}}.{filetype}?key={key}"),
            TileAddressingScheme::XYZ,
        )
    }

    /// Creates a source from a style tile URL template containing `{z}`, `{x}` and `{y}`, or
    /// `{bbox-epsg-3857}`.
    pub fn from_template(template: impl Into<String>, scheme: TileAddressingScheme) -> Self {
        Self {
            template: template.into(),
            scheme,
        }
    }

    /// Returns the URL template with unexpanded placeholders.
    pub fn template(&self) -> &str {
        &self.template
    }

    /// Returns the tile URL, or `None` when the coordinates are outside the world.
    pub fn format(&self, coords: &WorldTileCoords) -> Option<String> {
        format_template(&self.template, self.scheme, coords)
    }
}

impl Default for RasterSource {
    fn default() -> Self {
        Self::new(
            "https://api.maptiler.com/tiles/satellite-v2",
            "jpg",
            "qnePkfbGpMsLCi3KFBs3",
        )
    }
}

/// A GeoJSON source whose tiles are cut from an index the worker builds and shares.
#[derive(Clone, Debug)]
pub struct GeoJsonTileSource {
    /// The style's name for the source, which keys the shared index.
    pub name: String,
    /// The declaration: data, zoom range and id options.
    pub source: GeoJsonSource,
}

/// An image source, whose tiles are resampled from one picture instead of fetched.
#[derive(Clone, Debug)]
pub struct ImageTileSource {
    /// The declaration: the picture's URL and the corners it is stretched over.
    pub source: ImageSource,
}

/// Represents the tiles' different types of source.
#[derive(Clone, Debug)]
pub enum SourceType {
    /// GeoJSON document tiled by the worker instead of fetched per tile. Boxed because a
    /// declaration holding its data is many times the size of the other variants.
    GeoJson(Box<GeoJsonTileSource>),
    /// Raster image tile URL template.
    Raster(RasterSource),
    /// One picture resampled into raster tiles.
    Image(ImageTileSource),
    /// Vector tile URL template for geometry processing.
    Tessellate(TessellateSource),
}

impl SourceType {
    /// Returns the tile URL, or `None` when the coordinates are outside the world.
    pub fn format(&self, coords: &WorldTileCoords) -> Option<String> {
        match self {
            SourceType::GeoJson(_) | SourceType::Image(_) => None,
            SourceType::Raster(raster_source) => raster_source.format(coords),
            SourceType::Tessellate(tessellate_source) => tessellate_source.format(coords),
        }
    }
}

#[cfg(test)]
mod tests;
