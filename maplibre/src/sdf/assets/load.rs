//! Loads the glyph ranges and sprite images referenced by visible tile features.
use super::{AtlasBuilder, AtlasEntry, SymbolAtlas};
use crate::{
    io::source_client::{HttpClient, SourceClient},
    style::{layer::LayerPaint, Style},
    vector::feature_properties,
};
use geozero::mvt::Message;
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    sync::Arc,
};

type GlyphRequests = HashMap<String, BTreeSet<u32>>;

/// Fetches assets through the map's cached HTTP client before worker-side symbol layout.
pub async fn load_symbol_assets<HC: HttpClient>(
    client: &SourceClient<HC>,
    style: &Style,
    data: &[u8],
    zoom: f64,
) -> Arc<SymbolAtlas> {
    let (fonts, icons) = requests(style, data, zoom);
    let mut builder = AtlasBuilder::new();
    for (font, characters) in fonts {
        let ranges: BTreeSet<_> = characters.iter().map(|code| (code / 256) * 256).collect();
        for range in ranges {
            let bytes = if let Some(template) = &style.glyphs {
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
                let url = template
                    .replace("{fontstack}", &encoded.replace('+', "%20"))
                    .replace("{range}", &format!("{range}-{}", range + 255));
                match client.fetch_url(&url).await {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        tracing::warn!(%url, error = %error.describe(), "symbol glyph range unavailable");
                        if range != 0 {
                            continue;
                        }
                        include_bytes!("../../../../data/0-255.pbf").to_vec()
                    }
                }
            } else {
                if range != 0 {
                    continue;
                }
                include_bytes!("../../../../data/0-255.pbf").to_vec()
            };
            if let Err(error) = builder.glyph_subset(&font, &bytes, Some(&characters)) {
                tracing::warn!(%font, range, %error, "invalid glyph range");
            }
        }
    }
    if !icons.is_empty() {
        for (prefix, url) in sprite_sources(style) {
            load_sprites(client, &mut builder, &prefix, &url, &icons).await;
        }
    }
    builder.finish()
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
) {
    let json_url = sprite_url(url, "json");
    let png_url = sprite_url(url, "png");
    let Some(json) = fetch_asset(client, &json_url).await else {
        return;
    };
    let Some(png) = fetch_asset(client, &png_url).await else {
        return;
    };
    let document: HashMap<String, serde_json::Value> = match serde_json::from_slice(&json) {
        Ok(document) => document,
        Err(error) => {
            tracing::warn!(%json_url, %error, "invalid sprite metadata");
            return;
        }
    };
    let image = match image::load_from_memory(&png) {
        Ok(image) => image.to_rgba8(),
        Err(error) => {
            tracing::warn!(%png_url, %error, "invalid sprite image");
            return;
        }
    };
    pack_sprites(builder, prefix, wanted, document, &image);
}

fn pack_sprites(
    builder: &mut AtlasBuilder,
    prefix: &str,
    wanted: &HashSet<String>,
    document: HashMap<String, serde_json::Value>,
    image: &image::RgbaImage,
) {
    for (name, value) in document {
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
            },
        );
    }
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

async fn fetch_asset<HC: HttpClient>(client: &SourceClient<HC>, url: &str) -> Option<Vec<u8>> {
    match client.fetch_url(url).await {
        Ok(bytes) => Some(bytes),
        Err(error) => {
            tracing::warn!(%url,error=%error.describe(),"symbol asset unavailable");
            None
        }
    }
}

#[cfg(test)]
mod tests;
