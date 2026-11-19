//! Loads the glyph ranges and sprite images referenced by visible tile features.
use std::sync::Arc;

use rustc_hash::FxHashSet as HashSet;

use super::{
    cache::{AssetFailure, SpriteSheet},
    provider::{ImageProviders, Resolved},
    AtlasBuilder, AtlasEntry, SymbolAtlas,
};
use crate::{
    io::source_client::{HttpClient, SourceClient},
    sdf::glyphs::Glyphs,
    style::{layer::StyleLayer, Style, StyleImage},
};

mod local_fonts;
mod prefetch;
mod requests;
pub(crate) use prefetch::spawn_style_symbol_prefetch;
pub use prefetch::spawn_worker_symbol_prefetch;
#[cfg(test)]
use requests::requests;
use requests::symbol_requests;
pub(crate) use requests::{symbol_requests_from_tile, SymbolRequests};

#[cfg(not(target_arch = "wasm32"))]
const BUNDLED_LATIN: &[u8] = include_bytes!("../../../../data/0-255.pbf");

/// A symbol asset could not be loaded, and a later attempt can succeed.
#[derive(Debug, thiserror::Error)]
#[error("symbol asset {url} is temporarily unavailable: {reason}")]
pub struct SymbolAssetError {
    /// The URL that failed.
    pub url: String,
    /// The transport or server error.
    pub reason: String,
}

/// The style's symbol settings every source shares: where glyphs and sprites come from, and
/// the images a host added. Which layers want them is a source's own business.
#[derive(Clone, Copy, Debug)]
pub struct SymbolAssetConfig<'a> {
    /// The glyph URL template.
    pub glyphs: Option<&'a str>,
    /// The style's `sprite`, one URL or a list of `{id, url}`.
    pub sprite: Option<&'a serde_json::Value>,
    /// Images a host added to the style.
    pub images: &'a rustc_hash::FxHashMap<String, StyleImage>,
    /// Device pixels per layout pixel of the display, which provided images are made for.
    pub pixel_ratio: f32,
    /// Full style used to resolve `promoteId` and `feature-state` while scanning symbol requests.
    pub(crate) style: Option<&'a Style>,
}

impl<'a> SymbolAssetConfig<'a> {
    /// The symbol settings of `style`, for a display of one device pixel per layout pixel.
    pub fn of(style: &'a Style) -> Self {
        Self {
            glyphs: style.glyphs.as_deref(),
            sprite: style.sprite.as_ref(),
            images: &style.images,
            pixel_ratio: 1.0,
            style: Some(style),
        }
    }
}

/// A tile's atlas, and the provided images its labels name.
pub(crate) struct LoadedSymbolAssets {
    /// The glyphs and images that were ready.
    pub(crate) atlas: Arc<SymbolAtlas>,
    /// Every provided name the labels ask for, whether it was ready or not.
    pub(crate) provided: Vec<String>,
    /// The provided names with no answer yet, which a later load can pack once they have one.
    pub(crate) awaiting: Vec<String>,
}

/// Loads the glyph ranges and sprites that the features of one source's tile use under
/// `layers` through the client's shared cache.
///
/// `layers` are the source's own layers as its tile is cut, so they are the ones its symbols
/// are laid out with: a GeoJSON layer reads the source layer its tile is encoded under, and a
/// layer of another source never matches this tile's layers, whatever their names.
///
/// Missing or malformed assets are logged once and left out of the atlas. A transport or server
/// failure is returned so the caller can retry the tile instead of keeping an incomplete atlas.
pub async fn load_symbol_assets<'l, HC: HttpClient>(
    client: &SourceClient<HC>,
    config: SymbolAssetConfig<'_>,
    layers: impl IntoIterator<Item = &'l StyleLayer>,
    data: &[u8],
    zoom: f64,
) -> Result<Arc<SymbolAtlas>, SymbolAssetError> {
    Ok(
        load_symbol_assets_awaiting(client, config, layers, data, zoom)
            .await?
            .atlas,
    )
}

/// Like [`load_symbol_assets`], packing the provided images whose answers are known and
/// listing the ones still to come instead of waiting for them.
pub(crate) async fn load_symbol_assets_awaiting<'l, HC: HttpClient>(
    client: &SourceClient<HC>,
    config: SymbolAssetConfig<'_>,
    layers: impl IntoIterator<Item = &'l StyleLayer>,
    data: &[u8],
    zoom: f64,
) -> Result<LoadedSymbolAssets, SymbolAssetError> {
    let req = symbol_requests(layers, data, zoom, config.style);
    load_symbol_assets_for_requests(client, config, req).await
}

/// Like [`load_symbol_assets_awaiting`], reading symbol glyph and icon requests from an
/// already-decoded [`crate::vector::DecodedVectorTile`] so the worker does not decode the tile
/// payload again.
pub(crate) async fn load_symbol_assets_awaiting_from_tile<'l, HC: HttpClient>(
    client: &SourceClient<HC>,
    config: SymbolAssetConfig<'_>,
    layers: impl IntoIterator<Item = &'l StyleLayer>,
    tile: &crate::vector::DecodedVectorTile,
    zoom: f64,
) -> Result<LoadedSymbolAssets, SymbolAssetError> {
    let req = symbol_requests_from_tile(layers, tile, zoom, config.style);
    load_symbol_assets_for_requests(client, config, req).await
}

pub(crate) async fn load_symbol_assets_for_requests<HC: HttpClient>(
    client: &SourceClient<HC>,
    config: SymbolAssetConfig<'_>,
    req: SymbolRequests,
) -> Result<LoadedSymbolAssets, SymbolAssetError> {
    let mut builder = AtlasBuilder::new();
    let icons = req.icons;
    let sources = if icons.is_empty() {
        Vec::new()
    } else {
        sprite_sources(config.sprite)
    };
    let ((), fetched_sheets) = crate::util::async_join::try_join2(
        local_fonts::load_fonts_and_shaping(client, config, req.fonts, req.labels, &mut builder),
        crate::util::async_join::try_join_all_concurrent(
            sources
                .iter()
                .map(|(prefix, url)| fetch_sprite_sheet(client, prefix, url))
                .collect(),
        ),
    )
    .await?;
    for (prefix, sheet) in fetched_sheets.into_iter().flatten() {
        pack_sprites(&mut builder, prefix, &icons, &sheet);
    }
    for (name, image) in config.images {
        if icons.contains(name) {
            pack_style_image(&mut builder, name, image, None);
        }
    }
    // The sprite and the style's own images come first; a provider fills only the gaps.
    let providers = client.image_providers();
    let (provided, awaiting) = pack_provided(&mut builder, providers, &icons, config.pixel_ratio);
    for name in &icons {
        if !name.is_empty() && !builder.atlas.icons.contains_key(name) && !providers.provides(name)
        {
            providers.record_missing(name);
        }
    }
    Ok(LoadedSymbolAssets {
        atlas: builder.finish(),
        provided,
        awaiting,
    })
}

/// Packs the provided images among `icons` that nothing else supplied and whose answers are
/// known, returning every provided name and the ones without an answer yet.
fn pack_provided(
    builder: &mut AtlasBuilder,
    providers: &ImageProviders,
    icons: &HashSet<String>,
    pixel_ratio: f32,
) -> (Vec<String>, Vec<String>) {
    let mut provided: Vec<String> = icons
        .iter()
        .filter(|name| !builder.atlas.icons.contains_key(*name) && providers.provides(name))
        .cloned()
        .collect();
    provided.sort();
    let mut awaiting = Vec::new();
    for name in &provided {
        match providers.known(name, pixel_ratio).as_deref() {
            Some(Resolved::Image(image)) => {
                pack_style_image(builder, name, &image.image, image.anchor);
            }
            Some(Resolved::None) => {}
            None => awaiting.push(name.clone()),
        }
    }
    (provided, awaiting)
}

/// RGBA bytes with each colour channel scaled by its alpha.
fn premultiply(data: &[u8]) -> Vec<u8> {
    let mut out = data.to_vec();
    for pixel in out.as_chunks_mut::<4>().0 {
        let alpha = u32::from(pixel[3]);
        for channel in &mut pixel[..3] {
            *channel = ((u32::from(*channel) * alpha + 127) / 255) as u8;
        }
    }
    out
}

/// Packs an image the host added to the style, replacing a sprite icon of the same name.
/// `anchor`, in image pixels, is the point placed where the image's centre would be.
fn pack_style_image(
    builder: &mut AtlasBuilder,
    name: &str,
    image: &StyleImage,
    anchor: Option<[f32; 2]>,
) {
    if image.width == 0
        || image.height == 0
        || image.data.len() != image.width as usize * image.height as usize * 4
    {
        return;
    }
    let premultiplied;
    let data = if image.sdf {
        &image.data
    } else {
        premultiplied = premultiply(&image.data);
        &premultiplied
    };
    let Some(rect) = builder.pack(image.width, image.height, data) else {
        return;
    };
    let [shift_x, shift_y] = anchor.map_or([0.0, 0.0], |[x, y]| {
        [x - image.width as f32 / 2.0, y - image.height as f32 / 2.0]
    });
    builder.atlas.icons.insert(
        name.to_owned(),
        AtlasEntry {
            rect,
            metrics: [shift_x, shift_y, 0.0, image.pixel_ratio.max(0.01)],
            kind: if image.sdf { 2 } else { 1 },
            ..Default::default()
        },
    );
}

/// The range from the style's glyph server, or the bundled Latin range when that is all there is.
pub(super) async fn glyph_range<HC: HttpClient>(
    client: &SourceClient<HC>,
    template: Option<&str>,
    font: &str,
    range: u32,
) -> Result<Option<Arc<Glyphs>>, SymbolAssetError> {
    #[cfg(not(target_arch = "wasm32"))]
    let bundled = || client.assets().bundled_glyphs(BUNDLED_LATIN);
    let Some(template) = template else {
        #[cfg(not(target_arch = "wasm32"))]
        return Ok(if range == 0 {
            bundled().await.ok()
        } else {
            None
        });
        #[cfg(target_arch = "wasm32")]
        return Ok(None);
    };
    let url = glyph_url(template, font, range);
    match client.assets().glyphs(client, &url).await {
        Ok(glyphs) => Ok(Some(glyphs)),
        Err(AssetFailure::Retryable(reason)) => Err(SymbolAssetError { url, reason }),
        #[cfg(not(target_arch = "wasm32"))]
        Err(_) if range == 0 => Ok(bundled().await.ok()),
        Err(_) => Ok(None),
    }
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(super) fn glyph_url(template: &str, font: &str, range: u32) -> String {
    let encoded: String = font
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect();
    template
        .replace("{fontstack}", &encoded.replace('+', "%20"))
        .replace("{range}", &format!("{range}-{}", range + 255))
}

pub(super) fn sprite_sources(sprite: Option<&serde_json::Value>) -> Vec<(String, String)> {
    match sprite {
        Some(serde_json::Value::String(url)) => vec![(String::new(), url.clone())],
        Some(serde_json::Value::Array(sources)) => sources
            .iter()
            .filter_map(|source| {
                let id = source.get("id")?.as_str()?;
                // The sprite named `default` supplies the plain image names.
                let prefix = if id == "default" {
                    String::new()
                } else {
                    format!("{id}:")
                };
                Some((prefix, source.get("url")?.as_str()?.to_string()))
            })
            .collect(),
        _ => Vec::new(),
    }
}

pub(super) async fn fetch_sprite_sheet<'a, HC: HttpClient>(
    client: &SourceClient<HC>,
    prefix: &'a str,
    url: &str,
) -> Result<Option<(&'a str, std::sync::Arc<SpriteSheet>)>, SymbolAssetError> {
    let json_url = sprite_url(url, "json");
    let png_url = sprite_url(url, "png");
    match client.assets().sprites(client, &json_url, &png_url).await {
        Ok(sheet) => Ok(Some((prefix, sheet))),
        Err(AssetFailure::Retryable(reason)) => Err(SymbolAssetError {
            url: json_url,
            reason,
        }),
        Err(_) => Ok(None),
    }
}

fn pack_sprites(
    builder: &mut AtlasBuilder,
    prefix: &str,
    wanted: &HashSet<String>,
    sheet: &SpriteSheet,
) {
    let image = &sheet.image;
    for wanted_name in wanted {
        let Some(local_name) = wanted_name.strip_prefix(prefix) else {
            continue;
        };
        let Some(entry) = sheet.document.get(local_name) else {
            continue;
        };
        let (x, y, width, height) = (entry.x, entry.y, entry.width, entry.height);
        // Colour icons are stored premultiplied, so filtering them never darkens an edge with
        // the colour of the clear texels around it.
        let Some(rect) = builder.pack_cropped_rgba(image, [x, y, width, height], !entry.sdf) else {
            continue;
        };
        builder.atlas.icons.insert(
            wanted_name.clone(),
            AtlasEntry {
                rect,
                metrics: [0.0, 0.0, 0.0, entry.pixel_ratio],
                kind: if entry.sdf { 2 } else { 1 },
                stretch: entry.stretch.clone(),
            },
        );
    }
}

pub(super) fn sprite_url(url: &str, extension: &str) -> String {
    let (path, query) = url
        .split_once('?')
        .map_or((url, None), |(path, query)| (path, Some(query)));
    format!(
        "{path}.{extension}{}",
        query.map_or_else(String::new, |query| format!("?{query}"))
    )
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
