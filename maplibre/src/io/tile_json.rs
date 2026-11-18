//! Resolves TileJSON source URLs into tile URL templates before tiles are requested.

#[cfg(any(test, not(target_arch = "wasm32")))]
use serde::Deserialize;

use crate::{
    io::source_client::{HttpClient, SourceClient},
    style::{
        source::{DemEncoding, RasterDemSource, Source, TileAddressingScheme, VectorSource},
        Style,
    },
    util::json::extract::{
        field_err, json_opt_bounds4, json_opt_str, json_opt_string, json_opt_string_vec,
        json_opt_u32, json_opt_u8, missing_field_err,
    },
};

/// The subset of a TileJSON document needed to address tiles.
#[derive(Debug, PartialEq)]
#[cfg_attr(any(test, not(target_arch = "wasm32")), derive(Deserialize))]
pub struct TileJson {
    /// Tile URL templates.
    pub tiles: Vec<String>,
    /// Lowest zoom level with tiles.
    #[cfg_attr(any(test, not(target_arch = "wasm32")), serde(default))]
    pub minzoom: Option<u8>,
    /// Highest zoom level with tiles.
    #[cfg_attr(any(test, not(target_arch = "wasm32")), serde(default))]
    pub maxzoom: Option<u8>,
    /// Bounds in which tiles are available.
    #[cfg_attr(any(test, not(target_arch = "wasm32")), serde(default))]
    pub bounds: Option<(f64, f64, f64, f64)>,
    /// Credit the tiles' provider asks for.
    #[cfg_attr(any(test, not(target_arch = "wasm32")), serde(default))]
    pub attribution: Option<String>,
    /// Y-axis convention of the tile URLs.
    #[cfg_attr(any(test, not(target_arch = "wasm32")), serde(default))]
    pub scheme: Option<TileAddressingScheme>,
    /// Edge length of a tile in pixels.
    #[cfg_attr(
        any(test, not(target_arch = "wasm32")),
        serde(default, rename = "tileSize")
    )]
    pub tile_size: Option<u32>,
    /// Elevation packing of DEM tiles.
    #[cfg_attr(any(test, not(target_arch = "wasm32")), serde(default))]
    pub encoding: Option<DemEncoding>,
}

impl TileJson {
    /// Parses a [`TileJson`] subset from UTF-8 JSON bytes without `serde::Deserialize` bloat.
    pub fn from_json_slice(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        let value: serde_json::Value = serde_json::from_slice(bytes)?;
        Self::from_json_value(&value)
    }

    /// Parses a [`TileJson`] subset from a parsed JSON [`serde_json::Value`].
    pub fn from_json_value(value: &serde_json::Value) -> Result<Self, serde_json::Error> {
        let Some(map) = value.as_object() else {
            return Err(field_err("tilejson", "expected object"));
        };
        let tiles = json_opt_string_vec(map, "tiles")?.ok_or_else(|| missing_field_err("tiles"))?;
        let scheme = match json_opt_str(map, "scheme")? {
            None => None,
            Some("xyz") => Some(TileAddressingScheme::XYZ),
            Some("tms") => Some(TileAddressingScheme::TMS),
            Some(_) => return Err(field_err("scheme", "expected `xyz` or `tms`")),
        };
        let encoding = match json_opt_str(map, "encoding")? {
            None => None,
            Some("mapbox") => Some(DemEncoding::Mapbox),
            Some("terrarium") => Some(DemEncoding::Terrarium),
            Some("custom") => Some(DemEncoding::Custom),
            Some(_) => {
                return Err(field_err(
                    "encoding",
                    "expected `mapbox`, `terrarium`, or `custom`",
                ))
            }
        };
        Ok(Self {
            tiles,
            minzoom: json_opt_u8(map, "minzoom")?,
            maxzoom: json_opt_u8(map, "maxzoom")?,
            bounds: json_opt_bounds4(map, "bounds")?,
            attribution: json_opt_string(map, "attribution")?,
            scheme,
            tile_size: json_opt_u32(map, "tileSize")?,
            encoding,
        })
    }
}

/// The addressing fields a TileJSON document can fill in, shared by tile and DEM sources.
struct TileJsonFields<'a> {
    tiles: &'a mut Option<Vec<String>>,
    minzoom: &'a mut Option<u8>,
    maxzoom: &'a mut Option<u8>,
    bounds: &'a mut Option<(f64, f64, f64, f64)>,
    attribution: &'a mut Option<String>,
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
        if self.attribution.is_none() {
            *self.attribution = tile_json.attribution;
        }
    }
}

/// Fills the tile URLs, zoom range, attribution, scheme and tile size a source leaves
/// unspecified from its TileJSON document, as GL JS's `loadTileJson` does.
pub fn apply_tile_json(source: &mut VectorSource, tile_json: TileJson) {
    if source.scheme.is_none() {
        source.scheme = tile_json.scheme;
    }
    if source.tile_size.is_none() {
        source.tile_size = tile_json.tile_size;
    }
    TileJsonFields {
        tiles: &mut source.tiles,
        minzoom: &mut source.minzoom,
        maxzoom: &mut source.maxzoom,
        bounds: &mut source.bounds,
        attribution: &mut source.attribution,
    }
    .apply(tile_json);
}

/// Fills the tile URLs, zoom range, tile size and encoding of a DEM source from its TileJSON
/// document; a document such as Mapterhorn's declares `"encoding": "terrarium"`.
pub fn apply_tile_json_to_dem(source: &mut RasterDemSource, tile_json: TileJson) {
    if source.tile_size.is_none() {
        source.tile_size = tile_json.tile_size;
    }
    if source.encoding.is_none() {
        source.encoding = tile_json.encoding;
    }
    TileJsonFields {
        tiles: &mut source.tiles,
        minzoom: &mut source.minzoom,
        maxzoom: &mut source.maxzoom,
        bounds: &mut source.bounds,
        attribution: &mut source.attribution,
    }
    .apply(tile_json);
}

/// Returns the TileJSON URL of a source that declares `url` but no `tiles`.
fn pending_tile_json_url(source: &Source) -> Option<String> {
    let (tiles, url) = match source {
        Source::Vector(vector) | Source::Raster(vector) => (&vector.tiles, &vector.url),
        Source::RasterDem(dem) => (&dem.tiles, &dem.url),
        Source::GeoJson(_) | Source::Image(_) | Source::Video(_) | Source::Canvas(_) => {
            return None;
        }
    };
    tiles.is_none().then(|| url.clone()).flatten()
}

fn apply_tile_json_to_source(source: &mut Source, tile_json: TileJson) {
    match source {
        Source::Vector(vector) | Source::Raster(vector) => apply_tile_json(vector, tile_json),
        Source::RasterDem(dem) => apply_tile_json_to_dem(dem, tile_json),
        Source::GeoJson(_) | Source::Image(_) | Source::Video(_) | Source::Canvas(_) => {}
    }
}

/// Fetches the TileJSON of every tile source that declares a `url` but no `tiles`.
///
/// A source whose document cannot be fetched or parsed is left unchanged so its layers fall back
/// to the crate default source instead of failing the whole style.
///
/// A windowed map does this when its renderer initializes; a headless map's host awaits it
/// before `HeadlessMap::new`, passing the kernel's
/// source client. Nothing in it blocks, so it runs on a browser's event loop.
pub async fn resolve_tile_json_sources<HC: HttpClient>(
    style: &mut Style,
    client: &SourceClient<HC>,
) {
    let pending: Vec<(String, String, bool)> = style
        .sources
        .iter()
        .filter_map(|(name, source)| {
            pending_tile_json_url(source).map(|url| (name.clone(), url, source.is_volatile()))
        })
        .collect();
    if pending.is_empty() {
        return;
    }
    let results = crate::util::async_join::join_all_concurrent(
        pending
            .iter()
            .map(|(_, url, volatile)| client.fetch_source_url(url, *volatile)),
    )
    .await;
    for ((name, url, _), fetched) in pending.into_iter().zip(results) {
        let Some(source) = style.sources.get_mut(&name) else {
            continue;
        };
        match fetched {
            Ok(bytes) => match TileJson::from_json_slice(&bytes) {
                Ok(tile_json) => {
                    tracing::info!(source = %name, %url, "resolved TileJSON source");
                    apply_tile_json_to_source(source, tile_json);
                }
                Err(error) => {
                    tracing::warn!(source = %name, %url, %error, "TileJSON document is invalid");
                }
            },
            Err(error) => {
                tracing::warn!(
                    source = %name,
                    %url,
                    error = %error.describe(),
                    "TileJSON document is unreachable"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests;
