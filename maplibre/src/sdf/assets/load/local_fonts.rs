//! Font-face fetching, local CJK/fallback SDF rasterisation, and OpenType `harfrust` shaping
//! during tile symbol asset loading.

use std::collections::{BTreeSet, HashMap};

use super::{
    requests::GlyphRequests, AssetFailure, AtlasBuilder, SymbolAssetConfig, SymbolAssetError,
};
use crate::{
    font::{global_font_system, is_complex_shaping_codepoint, is_local_cjk_codepoint},
    io::source_client::{HttpClient, SourceClient},
    style::Style,
};

pub(super) async fn load_fonts_and_shaping<HC: HttpClient>(
    client: &SourceClient<HC>,
    config: SymbolAssetConfig<'_>,
    fonts: GlyphRequests,
    labels: HashMap<String, BTreeSet<String>>,
    builder: &mut AtlasBuilder,
) -> Result<(), SymbolAssetError> {
    if let Some(style) = config.style {
        ensure_style_font_faces(client, style).await?;
    }
    let ideograph_family = config
        .style
        .and_then(|style| style.local_ideograph_font_family.as_deref());

    for (font, characters) in fonts {
        let mut remote_chars = BTreeSet::new();
        let mut local_cjk = Vec::new();
        let has_registered = {
            let mut fs = global_font_system();
            let registered = fs.has_registered_stack(&font);
            for &code in &characters {
                let Some(ch) = char::from_u32(code) else {
                    continue;
                };
                if registered {
                    if let Some((_, face)) = fs.resolve_face_for_char(&font, ch, false) {
                        if let Some(glyph) = face.rasterize_char(ch) {
                            builder.pack_rasterized_codepoint(&font, code, &glyph);
                            continue;
                        }
                    }
                }
                if ideograph_family.is_some() && is_local_cjk_codepoint(ch) {
                    local_cjk.push((code, ch));
                } else {
                    remote_chars.insert(code);
                }
            }
            for &(code, ch) in &local_cjk {
                if let Some(glyph) = fs.rasterize_local_char(ideograph_family, &font, ch) {
                    builder.pack_rasterized_codepoint(&font, code, &glyph);
                }
            }
            registered
        };

        let ranges: BTreeSet<u32> = remote_chars.iter().map(|code| (code / 256) * 256).collect();
        for range in ranges {
            if let Some(glyphs) = super::glyph_range(client, config.glyphs, &font, range).await? {
                builder.glyph_subset(&font, &glyphs, Some(&remote_chars));
            }
        }

        let mut fallback_used = false;
        if ideograph_family.is_some() && !remote_chars.is_empty() {
            let mut fs = global_font_system();
            for &code in &remote_chars {
                let already_packed = builder
                    .atlas
                    .glyphs
                    .get(&font)
                    .is_some_and(|map| map.contains_key(&code));
                if already_packed {
                    continue;
                }
                let Some(ch) = char::from_u32(code) else {
                    continue;
                };
                if let Some(glyph) = fs.rasterize_local_char(ideograph_family, &font, ch) {
                    builder.pack_rasterized_codepoint(&font, code, &glyph);
                    fallback_used = true;
                }
            }
        }

        if let Some(stack_labels) = labels.get(&font) {
            shape_stack_labels(
                &font,
                ideograph_family,
                has_registered,
                fallback_used,
                stack_labels,
                builder,
            );
        }
    }
    Ok(())
}

async fn ensure_style_font_faces<HC: HttpClient>(
    client: &SourceClient<HC>,
    style: &Style,
) -> Result<(), SymbolAssetError> {
    let Some(faces) = style.font_faces.as_ref() else {
        return Ok(());
    };
    for rule in &faces.0 {
        let already_loaded = {
            let fs = global_font_system();
            rule.urls
                .iter()
                .any(|url| fs.has_registered_rule(rule, url))
        };
        if already_loaded {
            continue;
        }
        for url in &rule.urls {
            match super::super::fetch(client, url, "font face").await {
                Ok(bytes) => {
                    let registered = global_font_system()
                        .register_font_face_rule(rule, url, bytes)
                        .ok();
                    if registered.is_some() {
                        break;
                    }
                }
                Err(AssetFailure::Retryable(reason)) => {
                    return Err(SymbolAssetError {
                        url: url.clone(),
                        reason,
                    });
                }
                Err(_) => {}
            }
        }
    }
    Ok(())
}

fn shape_stack_labels(
    font: &str,
    ideograph_family: Option<&str>,
    has_registered: bool,
    fallback_used: bool,
    stack_labels: &BTreeSet<String>,
    builder: &mut AtlasBuilder,
) {
    if !has_registered && !(ideograph_family.is_some() && fallback_used) {
        return;
    }
    let mut fs = global_font_system();
    for label in stack_labels {
        let shaped = if has_registered {
            let all_registered = label
                .chars()
                .filter(|c| !c.is_whitespace())
                .all(|ch| fs.resolve_face_for_char(font, ch, false).is_some());
            if all_registered {
                fs.shape_text(font, label, false)
            } else if ideograph_family.is_some() {
                let stack = combined_stack(font, ideograph_family);
                fs.shape_text(&stack, label, true)
            } else {
                None
            }
        } else if label.chars().any(is_complex_shaping_codepoint) {
            let stack = combined_stack(font, ideograph_family);
            fs.shape_text(&stack, label, true)
        } else {
            None
        };

        let Some(shaped) = shaped.filter(|s| !s.is_empty()) else {
            continue;
        };
        let mut all_packed = true;
        for sg in &shaped {
            let key = sg.atlas_key();
            if !builder.atlas.face_glyphs.contains_key(&key) {
                if let Some(glyph) = fs.rasterize_face_glyph(sg.face_id, sg.glyph_id) {
                    builder.pack_rasterized_face_glyph(key.clone(), &glyph);
                }
            }
            if !builder.atlas.face_glyphs.contains_key(&key) {
                all_packed = false;
                break;
            }
        }
        if all_packed {
            builder
                .atlas
                .shaped_runs
                .entry(font.to_owned())
                .or_default()
                .insert(label.clone(), shaped);
        }
    }
}

fn combined_stack(font: &str, ideograph_family: Option<&str>) -> String {
    match ideograph_family {
        Some(family) if !family.is_empty() && !font.is_empty() => format!("{font},{family}"),
        Some(family) if !family.is_empty() => family.to_owned(),
        _ => font.to_owned(),
    }
}
