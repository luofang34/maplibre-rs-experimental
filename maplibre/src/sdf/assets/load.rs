//! Loads the glyph ranges and sprite images referenced by visible tile features.
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    sync::Arc,
};

use geozero::mvt::Message;

use super::{
    cache::{AssetFailure, SpriteSheet},
    AtlasBuilder, AtlasEntry, IconStretch, SymbolAtlas, TextFit,
};
use crate::{
    io::source_client::{HttpClient, SourceClient},
    sdf::glyphs::Glyphs,
    style::{layer::LayerPaint, Style},
    vector::feature_properties,
};

const BUNDLED_LATIN: &[u8] = include_bytes!("../../../../data/0-255.pbf");

type GlyphRequests = HashMap<String, BTreeSet<u32>>;

/// A symbol asset could not be loaded, and a later attempt can succeed.
#[derive(Debug, thiserror::Error)]
#[error("symbol asset {url} is temporarily unavailable: {reason}")]
pub struct SymbolAssetError {
    /// The URL that failed.
    pub url: String,
    /// The transport or server error.
    pub reason: String,
}

/// Loads the glyph ranges and sprites the tile's features use through the client's shared cache.
///
/// Missing or malformed assets are logged once and left out of the atlas. A transport or server
/// failure is returned so the caller can retry the tile instead of keeping an incomplete atlas.
pub async fn load_symbol_assets<HC: HttpClient>(
    client: &SourceClient<HC>,
    style: &Style,
    data: &[u8],
    zoom: f64,
) -> Result<Arc<SymbolAtlas>, SymbolAssetError> {
    let (fonts, icons) = requests(style, data, zoom);
    let mut builder = AtlasBuilder::new();
    for (font, characters) in fonts {
        let ranges: BTreeSet<_> = characters.iter().map(|code| (code / 256) * 256).collect();
        for range in ranges {
            if let Some(glyphs) = glyph_range(client, style, &font, range).await? {
                builder.glyph_subset(&font, &glyphs, Some(&characters));
            }
        }
    }
    if !icons.is_empty() {
        for (prefix, url) in sprite_sources(style) {
            load_sprites(client, &mut builder, &prefix, &url, &icons).await?;
        }
    }
    Ok(builder.finish())
}

/// The range from the style's glyph server, or the bundled Latin range when that is all there is.
async fn glyph_range<HC: HttpClient>(
    client: &SourceClient<HC>,
    style: &Style,
    font: &str,
    range: u32,
) -> Result<Option<Arc<Glyphs>>, SymbolAssetError> {
    let bundled = || client.assets().bundled_glyphs(BUNDLED_LATIN);
    let Some(template) = &style.glyphs else {
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

fn requests(style: &Style, data: &[u8], zoom: f64) -> (GlyphRequests, HashSet<String>) {
    let mut fonts = GlyphRequests::new();
    let mut icons = HashSet::new();
    let Ok(tile) = geozero::mvt::Tile::decode(data) else {
        return (fonts, icons);
    };
    for layer in &style.layers {
        if layer.is_hidden() {
            continue;
        }
        let Some(LayerPaint::Symbol(paint)) = &layer.paint else {
            continue;
        };
        let Some(source) = tile
            .layers
            .iter()
            .find(|source| Some(&source.name) == layer.source_layer.as_ref())
        else {
            continue;
        };
        for feature in &source.features {
            let properties = feature_properties(source, feature);
            if let Some(filter) = &layer.filter {
                let Ok(filter) = crate::style::filter::Filter::parse(filter) else {
                    continue;
                };
                if !filter.evaluate(&crate::style::filter::FeatureContext {
                    properties: &properties,
                    geometry_type: crate::style::filter::GeometryType::from_mvt(
                        feature.r#type.unwrap_or_default(),
                    ),
                    id: feature
                        .id
                        .map(|id| crate::style::expression::Value::Number(id as f64)),
                    zoom,
                }) {
                    continue;
                }
            }
            if let Some(text) = paint.label(&properties, zoom) {
                fonts
                    .entry(paint.font_stack())
                    .or_default()
                    .extend(text.chars().map(|c| c as u32));
            }
            if let Some(icon) = paint
                .text("icon-image", &properties, zoom)
                .filter(|icon| !icon.is_empty())
            {
                icons.insert(icon);
            }
        }
    }
    (fonts, icons)
}

fn sprite_sources(style: &Style) -> Vec<(String, String)> {
    match &style.sprite {
        Some(serde_json::Value::String(url)) => vec![(String::new(), url.clone())],
        Some(serde_json::Value::Array(sources)) => sources
            .iter()
            .filter_map(|source| {
                Some((
                    format!("{}:", source.get("id")?.as_str()?),
                    source.get("url")?.as_str()?.to_string(),
                ))
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
        let Some(rect) = builder.pack(width, height, pixels.as_raw()) else {
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
