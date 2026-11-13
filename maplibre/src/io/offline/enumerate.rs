//! Style resource enumeration for offline region downloads.

use std::collections::BTreeSet;

use super::region::OfflineRegionDefinition;
use crate::{
    io::{
        source_client::ResourceKind,
        source_type::{RasterSource, SourceType, TessellateSource},
    },
    style::{
        layer::LayerPaint,
        source::{GeoJsonData, Source, TileAddressingScheme},
        Style,
    },
};

/// One concrete URL and its [`ResourceKind`] required by an offline region.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OfflineResourceDescriptor {
    /// Resolved URL (prior to [`crate::io::source_client::RequestTransform`]).
    pub url: String,
    /// Kind of map resource (`Source`, `SpriteJson`, `SpriteImage`, `Glyphs`, `Image`, `Tile`).
    pub kind: ResourceKind,
    /// Whether HTTP `404 Not Found` is acceptable (e.g. sparse tiles or optional `@2x` sprites).
    pub optional_on_404: bool,
}

/// Returns `(source_name, tilejson_url)` pairs for non-volatile sources whose `tiles` are not yet resolved.
pub fn pending_non_volatile_tile_json_urls(style: &Style) -> Vec<(String, String)> {
    let mut pending: Vec<(String, String)> = style
        .sources
        .iter()
        .filter(|(_, src)| !src.is_volatile())
        .filter_map(|(name, src)| {
            let (tiles, url) = match src {
                Source::Vector(v) | Source::Raster(v) => (&v.tiles, &v.url),
                Source::RasterDem(d) => (&d.tiles, &d.url),
                Source::GeoJson(_) | Source::Image(_) => return None,
            };
            if tiles.is_none() {
                url.as_ref().map(|u| (name.clone(), u.clone()))
            } else {
                None
            }
        })
        .collect();
    pending.sort_by(|a, b| a.0.cmp(&b.0));
    pending
}

/// Deterministically enumerates all non-volatile sprites, `0-255` glyph ranges, source documents,
/// and tile URLs required to render `style` offline within `definition`.
pub fn enumerate_style_resources(
    style: &Style,
    definition: &OfflineRegionDefinition,
) -> Vec<OfflineResourceDescriptor> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out: Vec<OfflineResourceDescriptor> = Vec::new();
    let mut push_unique = |url: String, kind: ResourceKind, optional_on_404: bool| {
        if !url.is_empty() && !url.starts_with("data:") && seen.insert(url.clone()) {
            out.push(OfflineResourceDescriptor {
                url,
                kind,
                optional_on_404,
            });
        }
    };

    for sprite_base in extract_sprite_base_urls(style.sprite.as_ref()) {
        push_unique(
            format_sprite_url(&sprite_base, "", "json"),
            ResourceKind::SpriteJson,
            false,
        );
        push_unique(
            format_sprite_url(&sprite_base, "", "png"),
            ResourceKind::SpriteImage,
            false,
        );
        if definition.pixel_ratio > 1.0 {
            push_unique(
                format_sprite_url(&sprite_base, "@2x", "json"),
                ResourceKind::SpriteJson,
                true,
            );
            push_unique(
                format_sprite_url(&sprite_base, "@2x", "png"),
                ResourceKind::SpriteImage,
                true,
            );
        }
    }

    if let Some(glyph_template) = style.glyphs.as_deref() {
        let mut fonts: BTreeSet<String> = style
            .layers
            .iter()
            .filter_map(|layer| match &layer.paint {
                Some(LayerPaint::Symbol(paint)) => Some(paint.font_stack()),
                _ => None,
            })
            .collect();
        if fonts.is_empty() {
            fonts.insert("Open Sans Regular".to_owned());
        }
        for font in fonts {
            push_unique(
                format_glyph_url(glyph_template, &font, 0),
                ResourceKind::Glyphs,
                false,
            );
        }
    }

    let mut source_names: Vec<&String> = style.sources.keys().collect();
    source_names.sort();
    for name in source_names {
        let Some(source) = style.sources.get(name) else {
            continue;
        };
        if source.is_volatile() {
            continue;
        }
        enumerate_source(source, definition, &mut push_unique);
    }

    out
}

fn enumerate_source(
    source: &Source,
    definition: &OfflineRegionDefinition,
    push_unique: &mut impl FnMut(String, ResourceKind, bool),
) {
    match source {
        Source::GeoJson(geojson) => {
            if let GeoJsonData::Url(url) = &geojson.data {
                push_unique(url.clone(), ResourceKind::Source, false);
            }
        }
        Source::Image(img) => {
            push_unique(img.url.clone(), ResourceKind::Image, false);
        }
        Source::Vector(vec_src) => {
            if let Some(url) = &vec_src.url {
                push_unique(url.clone(), ResourceKind::Source, false);
            }
            if let Some(first_tpl) = vec_src.tiles.as_ref().and_then(|t| t.first()) {
                let scheme = vec_src.scheme.unwrap_or(TileAddressingScheme::XYZ);
                let st = SourceType::Tessellate(TessellateSource::from_template(first_tpl, scheme));
                for coords in definition.tile_pyramid_for_source(
                    vec_src.minzoom,
                    vec_src.maxzoom,
                    vec_src.bounds,
                ) {
                    if let Some(tile_url) = st.format(&coords) {
                        push_unique(tile_url, ResourceKind::Tile, true);
                    }
                }
            }
        }
        Source::Raster(ras_src) => {
            if let Some(url) = &ras_src.url {
                push_unique(url.clone(), ResourceKind::Source, false);
            }
            if let Some(first_tpl) = ras_src.tiles.as_ref().and_then(|t| t.first()) {
                let scheme = ras_src.scheme.unwrap_or(TileAddressingScheme::XYZ);
                let st = SourceType::Raster(RasterSource::from_template(first_tpl, scheme));
                for coords in definition.tile_pyramid_for_source(
                    ras_src.minzoom,
                    ras_src.maxzoom,
                    ras_src.bounds,
                ) {
                    if let Some(tile_url) = st.format(&coords) {
                        push_unique(tile_url, ResourceKind::Tile, true);
                    }
                }
            }
        }
        Source::RasterDem(dem_src) => {
            if let Some(url) = &dem_src.url {
                push_unique(url.clone(), ResourceKind::Source, false);
            }
            if let Some(first_tpl) = dem_src.tiles.as_ref().and_then(|t| t.first()) {
                let st = SourceType::Raster(RasterSource::from_template(
                    first_tpl,
                    TileAddressingScheme::XYZ,
                ));
                for coords in definition.tile_pyramid_for_source(
                    dem_src.minzoom,
                    dem_src.maxzoom,
                    dem_src.bounds,
                ) {
                    if let Some(tile_url) = st.format(&coords) {
                        push_unique(tile_url, ResourceKind::Tile, true);
                    }
                }
            }
        }
    }
}

fn extract_sprite_base_urls(sprite: Option<&serde_json::Value>) -> Vec<String> {
    match sprite {
        Some(serde_json::Value::String(url)) => vec![url.clone()],
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .filter_map(|entry| entry.get("url")?.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

fn format_sprite_url(base: &str, scale_suffix: &str, extension: &str) -> String {
    let (path, query) = base
        .split_once('?')
        .map_or((base, None), |(p, q)| (p, Some(q)));
    match query {
        Some(q) => format!("{path}{scale_suffix}.{extension}?{q}"),
        None => format!("{path}{scale_suffix}.{extension}"),
    }
}

fn format_glyph_url(template: &str, font: &str, range_start: u32) -> String {
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
        .replace("{range}", &format!("{range_start}-{}", range_start + 255))
}
