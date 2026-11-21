//! Font-face fetching, local CJK/fallback SDF rasterisation, and OpenType `harfrust` shaping
//! during tile symbol asset loading.

use std::sync::Arc;

use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

use super::{
    requests::GlyphRequests, AssetFailure, AtlasBuilder, SymbolAssetConfig, SymbolAssetError,
};
use crate::{
    font::{
        global_font_system, is_complex_shaping_codepoint, is_local_cjk_codepoint, RasterizedGlyph,
    },
    io::source_client::{HttpClient, SourceClient},
    sdf::glyphs::Glyphs,
    style::Style,
};

struct PendingFont {
    font: String,
    has_registered: bool,
    registered_glyphs: Vec<(u32, RasterizedGlyph)>,
    remote_chars: HashSet<u32>,
    local_cjk: Vec<(u32, char)>,
    range_start: usize,
    range_len: usize,
}

/// Fetched remote glyph ranges and pending local-font jobs ready for synchronous atlas packing.
pub(super) struct FetchedFontInputs {
    pending_fonts: Vec<PendingFont>,
    fetched_ranges: Vec<Option<Arc<Glyphs>>>,
    labels: HashMap<String, HashSet<String>>,
}

/// Fetches remote font-face files and `.pbf` glyph ranges without touching `AtlasBuilder` or
/// `OffscreenCanvas`, so network I/O overlaps base geometry tessellation with zero atlas memory.
pub(super) async fn fetch_font_inputs<HC: HttpClient>(
    client: &SourceClient<HC>,
    config: SymbolAssetConfig<'_>,
    fonts: GlyphRequests,
    labels: HashMap<String, HashSet<String>>,
) -> Result<FetchedFontInputs, SymbolAssetError> {
    if let Some(style) = config.style {
        ensure_style_font_faces(client, style).await?;
    }
    let ideograph_family = config
        .style
        .and_then(|style| style.local_ideograph_font_family.as_deref());

    let mut pending_fonts = Vec::with_capacity(fonts.len());
    let mut range_jobs: Vec<(usize, u32)> = Vec::new();

    for (font, characters) in fonts {
        let mut remote_chars = HashSet::default();
        let mut local_cjk = Vec::new();
        let mut registered_glyphs = Vec::new();
        let has_registered = global_font_system().has_registered_stack(&font);

        if has_registered {
            let mut fs = global_font_system();
            for &code in &characters {
                let Some(ch) = char::from_u32(code) else {
                    continue;
                };
                if let Some((_, face)) = fs.resolve_face_for_char(&font, ch, false) {
                    if let Some(glyph) = face.rasterize_char(ch) {
                        registered_glyphs.push((code, glyph));
                        continue;
                    }
                }
                if ideograph_family.is_some() && is_local_cjk_codepoint(ch) {
                    local_cjk.push((code, ch));
                } else {
                    remote_chars.insert(code);
                }
            }
        } else {
            for &code in &characters {
                let Some(ch) = char::from_u32(code) else {
                    continue;
                };
                if ideograph_family.is_some() && is_local_cjk_codepoint(ch) {
                    local_cjk.push((code, ch));
                } else {
                    remote_chars.insert(code);
                }
            }
        }

        let mut ranges: Vec<u32> = remote_chars.iter().map(|code| (code / 256) * 256).collect();
        ranges.sort_unstable();
        ranges.dedup();
        let font_idx = pending_fonts.len();
        let range_start = range_jobs.len();
        for range in ranges {
            range_jobs.push((font_idx, range));
        }
        let range_len = range_jobs.len() - range_start;
        pending_fonts.push(PendingFont {
            font,
            has_registered,
            registered_glyphs,
            remote_chars,
            local_cjk,
            range_start,
            range_len,
        });
    }

    let fetched_ranges = crate::util::async_join::try_join_all_concurrent(
        range_jobs
            .iter()
            .map(|&(font_idx, range)| {
                super::glyph_range(client, config.glyphs, &pending_fonts[font_idx].font, range)
            })
            .collect(),
    )
    .await?;

    Ok(FetchedFontInputs {
        pending_fonts,
        fetched_ranges,
        labels,
    })
}

/// Packs fetched `.pbf` glyphs, local CJK SDF rasterizations, and shaped glyph runs into `builder`.
pub(super) fn pack_font_inputs(
    config: SymbolAssetConfig<'_>,
    inputs: FetchedFontInputs,
    builder: &mut AtlasBuilder,
) {
    let ideograph_family = config
        .style
        .and_then(|style| style.local_ideograph_font_family.as_deref());
    let FetchedFontInputs {
        pending_fonts,
        fetched_ranges,
        labels,
    } = inputs;

    for pending in &pending_fonts {
        for (code, glyph) in &pending.registered_glyphs {
            builder.pack_rasterized_codepoint(&pending.font, *code, glyph);
        }
        if !pending.local_cjk.is_empty() {
            #[cfg(target_arch = "wasm32")]
            if pending.has_registered {
                let mut fs = global_font_system();
                for &(code, ch) in &pending.local_cjk {
                    if let Some(glyph) =
                        fs.rasterize_local_char(ideograph_family, &pending.font, ch)
                    {
                        builder.pack_rasterized_codepoint(&pending.font, code, &glyph);
                    }
                }
            } else {
                for (code, glyph) in crate::font::rasterize_canvas2d_chars_sdf(
                    ideograph_family,
                    &pending.font,
                    &pending.local_cjk,
                ) {
                    builder.pack_rasterized_codepoint(&pending.font, code, &glyph);
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let mut fs = global_font_system();
                for &(code, ch) in &pending.local_cjk {
                    if let Some(glyph) =
                        fs.rasterize_local_char(ideograph_family, &pending.font, ch)
                    {
                        builder.pack_rasterized_codepoint(&pending.font, code, &glyph);
                    }
                }
            }
        }
    }

    for PendingFont {
        font,
        has_registered,
        remote_chars,
        range_start,
        range_len,
        ..
    } in pending_fonts
    {
        let mut had_any_pbf = false;
        if let Some(slice) = fetched_ranges.get(range_start..range_start + range_len) {
            for glyphs in slice.iter().flatten() {
                had_any_pbf = true;
                builder.glyph_subset(&font, glyphs, Some(&remote_chars));
            }
        }

        #[cfg(target_arch = "wasm32")]
        if (config.glyphs.is_none() || !had_any_pbf) && !remote_chars.is_empty() {
            let fallback: Vec<(u32, char)> = remote_chars
                .iter()
                .filter_map(|&code| {
                    let already = builder
                        .atlas
                        .glyphs
                        .get(&font)
                        .is_some_and(|map| map.contains_key(&code));
                    (!already)
                        .then(|| char::from_u32(code).map(|ch| (code, ch)))
                        .flatten()
                })
                .collect();
            if !fallback.is_empty() {
                let family = ideograph_family.or(Some("sans-serif"));
                for (code, glyph) in
                    crate::font::rasterize_canvas2d_chars_sdf(family, &font, &fallback)
                {
                    builder.pack_rasterized_codepoint(&font, code, &glyph);
                }
            }
        }
        let _ = had_any_pbf;

        let mut fallback_used = false;
        if (has_registered || cfg!(not(target_arch = "wasm32")))
            && ideograph_family.is_some()
            && !remote_chars.is_empty()
        {
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
    stack_labels: &HashSet<String>,
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

        if has_registered {
            let v_shaped = {
                let all_registered = label
                    .chars()
                    .filter(|ch| !ch.is_whitespace() && !ch.is_control())
                    .all(|ch| fs.resolve_face_for_char(font, ch, false).is_some());
                if all_registered {
                    fs.shape_text_vertical(font, label, false)
                } else if ideograph_family.is_some() {
                    let stack = combined_stack(font, ideograph_family);
                    fs.shape_text_vertical(&stack, label, true)
                } else {
                    None
                }
            };
            if let Some(v_shaped) = v_shaped.filter(|s| !s.is_empty()) {
                let mut v_all_packed = true;
                for sg in &v_shaped {
                    let key = sg.atlas_key();
                    if !builder.atlas.face_glyphs.contains_key(&key) {
                        if let Some(glyph) = fs.rasterize_face_glyph(sg.face_id, sg.glyph_id) {
                            builder.pack_rasterized_face_glyph(key.clone(), &glyph);
                        }
                    }
                    if !builder.atlas.face_glyphs.contains_key(&key) {
                        v_all_packed = false;
                        break;
                    }
                }
                if v_all_packed {
                    builder
                        .atlas
                        .vertical_shaped_runs
                        .entry(font.to_owned())
                        .or_default()
                        .insert(label.clone(), v_shaped);
                }
            }
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
