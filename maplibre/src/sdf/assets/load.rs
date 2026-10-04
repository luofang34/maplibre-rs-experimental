//! Loads the glyph ranges and sprite images referenced by visible tile features.
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    sync::Arc,
};

use super::{
    cache::{AssetFailure, SpriteSheet},
    AtlasBuilder, AtlasEntry, IconStretch, SymbolAtlas, TextFit,
};
use crate::{
    io::source_client::{HttpClient, SourceClient},
    sdf::glyphs::Glyphs,
    style::{layer::StyleLayer, Style, StyleImage},
};

mod requests;
use requests::requests;

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
    pub images: &'a HashMap<String, StyleImage>,
}

impl<'a> SymbolAssetConfig<'a> {
    /// The symbol settings of `style`.
    pub fn of(style: &'a Style) -> Self {
        Self {
            glyphs: style.glyphs.as_deref(),
            sprite: style.sprite.as_ref(),
            images: &style.images,
        }
    }
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
    let (fonts, icons) = requests(layers, data, zoom);
    let mut builder = AtlasBuilder::new();
    for (font, characters) in fonts {
        let ranges: BTreeSet<_> = characters.iter().map(|code| (code / 256) * 256).collect();
        for range in ranges {
            if let Some(glyphs) = glyph_range(client, config.glyphs, &font, range).await? {
                builder.glyph_subset(&font, &glyphs, Some(&characters));
            }
        }
    }
    if !icons.is_empty() {
        for (prefix, url) in sprite_sources(config.sprite) {
            load_sprites(client, &mut builder, &prefix, &url, &icons).await?;
        }
    }
    for (name, image) in config.images {
        if icons.contains(name) {
            pack_style_image(&mut builder, name, image);
        }
    }
    Ok(builder.finish())
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
fn pack_style_image(builder: &mut AtlasBuilder, name: &str, image: &crate::style::StyleImage) {
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
    builder.atlas.icons.insert(
        name.to_owned(),
        AtlasEntry {
            rect,
            metrics: [0.0, 0.0, 0.0, image.pixel_ratio.max(0.01)],
            kind: if image.sdf { 2 } else { 1 },
            ..Default::default()
        },
    );
}

/// The range from the style's glyph server, or the bundled Latin range when that is all there is.
async fn glyph_range<HC: HttpClient>(
    client: &SourceClient<HC>,
    template: Option<&str>,
    font: &str,
    range: u32,
) -> Result<Option<Arc<Glyphs>>, SymbolAssetError> {
    let bundled = || client.assets().bundled_glyphs(BUNDLED_LATIN);
    let Some(template) = template else {
        return Ok(if range == 0 {
            bundled().await.ok()
        } else {
            None
        });
    };
    let url = glyph_url(template, font, range);
    match client.assets().glyphs(client, &url).await {
        Ok(glyphs) => Ok(Some(glyphs)),
        Err(AssetFailure::Retryable(reason)) => Err(SymbolAssetError { url, reason }),
        Err(_) if range == 0 => Ok(bundled().await.ok()),
        Err(_) => Ok(None),
    }
}

fn glyph_url(template: &str, font: &str, range: u32) -> String {
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

async fn load_sprites<HC: HttpClient>(
    client: &SourceClient<HC>,
    builder: &mut AtlasBuilder,
    prefix: &str,
    url: &str,
    wanted: &HashSet<String>,
) -> Result<(), SymbolAssetError> {
    let json_url = sprite_url(url, "json");
    let png_url = sprite_url(url, "png");
    match client.assets().sprites(client, &json_url, &png_url).await {
        Ok(sheet) => pack_sprites(builder, prefix, wanted, &sheet),
        Err(AssetFailure::Retryable(reason)) => {
            return Err(SymbolAssetError {
                url: json_url,
                reason,
            })
        }
        Err(_) => {}
    }
    Ok(())
}

fn pack_sprites(
    builder: &mut AtlasBuilder,
    prefix: &str,
    wanted: &HashSet<String>,
    sheet: &SpriteSheet,
) {
    let image = &sheet.image;
    for (name, value) in &sheet.document {
        let name = format!("{prefix}{name}");
        if !wanted.contains(&name) {
            continue;
        }
        let read = |key: &str| {
            value
                .get(key)
                .and_then(|value| value.as_u64())
                .and_then(|value| u32::try_from(value).ok())
        };
        let (Some(x), Some(y), Some(width), Some(height)) =
            (read("x"), read("y"), read("width"), read("height"))
        else {
            continue;
        };
        if width == 0
            || height == 0
            || x.saturating_add(width) > image.width()
            || y.saturating_add(height) > image.height()
        {
            continue;
        }
        let pixels = image::imageops::crop_imm(image, x, y, width, height).to_image();
        let sdf = value.get("sdf").and_then(|v| v.as_bool()).unwrap_or(false);
        // Colour icons are stored premultiplied, so filtering them never darkens an edge with
        // the colour of the clear texels around it.
        let packed = if sdf {
            pixels.into_raw()
        } else {
            premultiply(pixels.as_raw())
        };
        let Some(rect) = builder.pack(width, height, &packed) else {
            continue;
        };
        let ratio = value
            .get("pixelRatio")
            .and_then(|value| value.as_f64())
            .unwrap_or(1.0)
            .max(0.01) as f32;
        builder.atlas.icons.insert(
            name,
            AtlasEntry {
                rect,
                metrics: [0.0, 0.0, 0.0, ratio],
                kind: if value.get("sdf").and_then(|v| v.as_bool()).unwrap_or(false) {
                    2
                } else {
                    1
                },
                stretch: icon_stretch(value),
            },
        );
    }
}

fn ranges(value: &serde_json::Value, key: &str) -> Vec<[f32; 2]> {
    value
        .get(key)
        .and_then(|ranges| ranges.as_array())
        .into_iter()
        .flatten()
        .filter_map(|range| {
            let range = range.as_array()?;
            Some([
                range.first()?.as_f64()? as f32,
                range.get(1)?.as_f64()? as f32,
            ])
        })
        .collect()
}

fn text_fit(value: &serde_json::Value, key: &str) -> Option<TextFit> {
    match value.get(key)?.as_str()? {
        "stretchOrShrink" => Some(TextFit::StretchOrShrink),
        "stretchOnly" => Some(TextFit::StretchOnly),
        "proportional" => Some(TextFit::Proportional),
        _ => None,
    }
}

/// The stretch fields of a sprite entry, or `None` when it has none.
fn icon_stretch(value: &serde_json::Value) -> Option<Box<IconStretch>> {
    let stretch = IconStretch {
        stretch_x: ranges(value, "stretchX"),
        stretch_y: ranges(value, "stretchY"),
        content: value.get("content").and_then(|content| {
            let content = content.as_array()?;
            let read = |index: usize| content.get(index)?.as_f64().map(|edge| edge as f32);
            Some([read(0)?, read(1)?, read(2)?, read(3)?])
        }),
        text_fit_width: text_fit(value, "textFitWidth"),
        text_fit_height: text_fit(value, "textFitHeight"),
    };
    (stretch != IconStretch::default()).then(|| Box::new(stretch))
}

fn sprite_url(url: &str, extension: &str) -> String {
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
