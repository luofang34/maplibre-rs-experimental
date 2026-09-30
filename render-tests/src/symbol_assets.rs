//! Glyphs and sprites for fixtures, read from the harness assets instead of a server.

use std::{path::PathBuf, sync::Arc};

use maplibre::{
    io::source_client::{HttpClient, HttpSourceClient, SourceClient, SourceFetchError},
    sdf::assets::{load_symbol_assets, SymbolAtlas},
    style::Style,
};

/// Serves `local://glyphs/...` and `local://sprites/...` from `render-tests/src/assets`.
#[derive(Clone)]
struct LocalAssets {
    /// Device pixel ratio: at two or more, a sprite sheet's `@2x` variant is the one fetched.
    pixel_ratio: f64,
}

fn asset_path(url: &str) -> Option<PathBuf> {
    let relative = url.strip_prefix("local://")?;
    let (directory, rest) = relative.split_once('/')?;
    if !matches!(directory, "glyphs" | "sprites") {
        return None;
    }
    Some(
        PathBuf::from("render-tests/src/assets")
            .join(directory)
            .join(percent_decoded(rest)),
    )
}

fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let escape = (bytes[index] == b'%')
            .then(|| bytes.get(index + 1..index + 3))
            .flatten()
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escape {
            Some(byte) => {
                decoded.push(byte);
                index += 3;
            }
            None => {
                decoded.push(bytes[index]);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// The `@2x` file next to a sprite sheet's `.json` or `.png`, as GL JS requests on dense screens.
fn high_density_sprite(path: &std::path::Path) -> Option<PathBuf> {
    let extension = path.extension()?.to_str()?;
    if !matches!(extension, "json" | "png") || !path.starts_with("render-tests/src/assets/sprites")
    {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    Some(path.with_file_name(format!("{stem}@2x.{extension}")))
}

#[async_trait::async_trait]
impl HttpClient for LocalAssets {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        let missing = || SourceFetchError::not_found(url);
        let path = asset_path(url).ok_or_else(missing)?;
        if self.pixel_ratio >= 2.0 {
            if let Some(high_density) = high_density_sprite(&path) {
                if let Ok(bytes) = std::fs::read(high_density) {
                    return Ok(bytes);
                }
            }
        }
        std::fs::read(path).map_err(|_| missing())
    }
}

/// The atlas of the glyphs and sprites the tile's symbols use, or `None` when no symbol layer
/// draws from it. The loader is asynchronous but every read is a local file, so it is driven
/// to completion here.
pub(super) fn load_atlas_blocking(
    style: &Style,
    tile: &[u8],
    (zoom, pixel_ratio): (f64, f64),
) -> Result<Option<Arc<SymbolAtlas>>, String> {
    let client = SourceClient::new(HttpSourceClient::new(LocalAssets { pixel_ratio }));
    let atlas = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(load_symbol_assets(&client, style, tile, zoom))
    })
    .map_err(|error| format!("Cannot load symbol assets: {error:?}"))?;
    Ok(Some(atlas))
}
