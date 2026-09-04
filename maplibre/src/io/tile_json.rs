//! Resolves TileJSON source URLs into tile URL templates before tiles are requested.

use serde::Deserialize;

use crate::{
    io::source_client::{HttpClient, SourceClient},
    style::{
        source::{RasterDemSource, Source, VectorSource},
        Style,
    },
};

/// The subset of a TileJSON document needed to address tiles.
#[derive(Debug, Deserialize, PartialEq)]
pub struct TileJson {
    /// Tile URL templates.
    pub tiles: Vec<String>,
    /// Lowest zoom level with tiles.
    #[serde(default)]
    pub minzoom: Option<u8>,
    /// Highest zoom level with tiles.
    #[serde(default)]
    pub maxzoom: Option<u8>,
    /// Bounds in which tiles are available.
    #[serde(default)]
    pub bounds: Option<(f64, f64, f64, f64)>,
}

/// The addressing fields a TileJSON document can fill in, shared by tile and DEM sources.
struct TileJsonFields<'a> {
    tiles: &'a mut Option<Vec<String>>,
    minzoom: &'a mut Option<u8>,
    maxzoom: &'a mut Option<u8>,
    bounds: &'a mut Option<(f64, f64, f64, f64)>,
}

impl TileJsonFields<'_> {
    fn apply(self, tile_json: TileJson) {
        if self.tiles.is_none() {
            *self.tiles = Some(tile_json.tiles);
        }
        if self.minzoom.is_none() {
            *self.minzoom = tile_json.minzoom;
        }
        if self.maxzoom.is_none() {
            *self.maxzoom = tile_json.maxzoom;
        }
        if self.bounds.is_none() {
            *self.bounds = tile_json.bounds;
        }
    }
}

/// Fills the tile URLs and zoom range a source leaves unspecified from its TileJSON document.
pub fn apply_tile_json(source: &mut VectorSource, tile_json: TileJson) {
    TileJsonFields {
        tiles: &mut source.tiles,
        minzoom: &mut source.minzoom,
        maxzoom: &mut source.maxzoom,
        bounds: &mut source.bounds,
    }
    .apply(tile_json);
}

/// Fills the tile URLs and zoom range of a DEM source from its TileJSON document.
pub fn apply_tile_json_to_dem(source: &mut RasterDemSource, tile_json: TileJson) {
    TileJsonFields {
        tiles: &mut source.tiles,
        minzoom: &mut source.minzoom,
        maxzoom: &mut source.maxzoom,
        bounds: &mut source.bounds,
    }
    .apply(tile_json);
}

/// Returns the TileJSON URL of a source that declares `url` but no `tiles`.
fn pending_tile_json_url(source: &Source) -> Option<String> {
    let (tiles, url) = match source {
        Source::Vector(vector) | Source::Raster(vector) => (&vector.tiles, &vector.url),
        Source::RasterDem(dem) => (&dem.tiles, &dem.url),
        Source::GeoJson(_) => return None,
    };
    tiles.is_none().then(|| url.clone()).flatten()
}

fn apply_tile_json_to_source(source: &mut Source, tile_json: TileJson) {
    match source {
        Source::Vector(vector) | Source::Raster(vector) => apply_tile_json(vector, tile_json),
        Source::RasterDem(dem) => apply_tile_json_to_dem(dem, tile_json),
        Source::GeoJson(_) => {}
    }
}

/// Fetches the TileJSON of every tile source that declares a `url` but no `tiles`.
///
/// A source whose document cannot be fetched or parsed is left unchanged so its layers fall back
/// to the crate default source instead of failing the whole style.
pub async fn resolve_tile_json_sources<HC: HttpClient>(
    style: &mut Style,
    client: &SourceClient<HC>,
) {
    for (name, source) in &mut style.sources {
        let Some(url) = pending_tile_json_url(source) else {
            continue;
        };
        match client.fetch_url(&url).await {
            Ok(bytes) => match serde_json::from_slice::<TileJson>(&bytes) {
                Ok(tile_json) => {
                    tracing::info!(source = %name, %url, "resolved TileJSON source");
                    apply_tile_json_to_source(source, tile_json);
                }
                Err(error) => {
                    tracing::warn!(source = %name, %url, %error, "TileJSON document is invalid");
                }
            },
            Err(error) => {
                tracing::warn!(source = %name, %url, %error, "TileJSON document is unreachable");
            }
        }
    }
}

#[cfg(test)]
mod tests;
