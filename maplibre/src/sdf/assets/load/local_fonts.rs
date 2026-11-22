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
        let font_idx = pending_fonts.len();
        let range_start = range_jobs.len();
        let (pending, ranges) =
            classify_font_chars(font, &characters, ideograph_family, range_start);
        for range in ranges {
            range_jobs.push((font_idx, range));
        }
        pending_fonts.push(pending);
    }

    let fetched_ranges = crate::util::async_join::try_join_all_concurrent(
        range_jobs
            .iter()
            .filter_map(|&(font_idx, range)| {
                pending_fonts
                    .get(font_idx)
                    .map(|p| super::glyph_range(client, config.glyphs, &p.font, range))
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

fn classify_font_chars(
    font: String,
    characters: &HashSet<u32>,
    ideograph_family: Option<&str>,
    range_start: usize,
) -> (PendingFont, Vec<u32>) {
    let mut remote_chars = HashSet::default();
    let mut local_cjk = Vec::new();
    let mut registered_glyphs = Vec::new();
    let has_registered = global_font_system().has_registered_stack(&font);
    let mut fs = has_registered.then(global_font_system);

    for &code in characters {
        let Some(ch) = char::from_u32(code) else {
            continue;
        };
        if let Some(fs) = fs.as_mut() {
            if let Some(glyph) = fs
                .resolve_face_for_char(&font, ch, false)
                .and_then(|(_, face)| face.rasterize_char(ch))
            {
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

    let mut ranges: Vec<u32> = remote_chars.iter().map(|code| (code / 256) * 256).collect();
    ranges.sort_unstable();
    ranges.dedup();
    let pending = PendingFont {
        font,
        has_registered,
        registered_glyphs,
        remote_chars,
        local_cjk,
        range_start,
        range_len: ranges.len(),
    };
    (pending, ranges)
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
        pack_registered_and_cjk(pending, ideograph_family, builder);
    }

    for pending in pending_fonts {
        let slice = fetched_ranges
            .get(pending.range_start..pending.range_start + pending.range_len)
            .unwrap_or(&[]);
        let fallback_used =
            pack_pbf_and_local_fallback(config.glyphs, ideograph_family, &pending, slice, builder);
        if let Some(stack_labels) = labels.get(&pending.font) {
            shape_stack_labels(
                &pending.font,
                ideograph_family,
                pending.has_registered,
                fallback_used,
                stack_labels,
                builder,
            );
        }
    }
}

fn pack_registered_and_cjk(
    pending: &PendingFont,
    ideograph_family: Option<&str>,
    builder: &mut AtlasBuilder,
) {
    for (code, glyph) in &pending.registered_glyphs {
        builder.pack_rasterized_codepoint(&pending.font, *code, glyph);
    }
    if pending.local_cjk.is_empty() {
        return;
    }
    #[cfg(target_arch = "wasm32")]
    if !pending.has_registered {
        for (code, glyph) in crate::font::rasterize_canvas2d_chars_sdf(
            ideograph_family,
            &pending.font,
            &pending.local_cjk,
        ) {
            builder.pack_rasterized_codepoint(&pending.font, code, &glyph);
        }
        return;
    }
    let mut fs = global_font_system();
    for &(code, ch) in &pending.local_cjk {
        if let Some(glyph) = fs.rasterize_local_char(ideograph_family, &pending.font, ch) {
            builder.pack_rasterized_codepoint(&pending.font, code, &glyph);
        }
    }
}

fn pack_pbf_and_local_fallback(
    glyphs_url: Option<&str>,
    ideograph_family: Option<&str>,
    pending: &PendingFont,
    fetched_slice: &[Option<Arc<Glyphs>>],
    builder: &mut AtlasBuilder,
) -> bool {
    let mut had_any_pbf = false;
    for glyphs in fetched_slice.iter().flatten() {
        had_any_pbf = true;
        builder.glyph_subset(&pending.font, glyphs, Some(&pending.remote_chars));
    }

    #[cfg(target_arch = "wasm32")]
    if (glyphs_url.is_none() || !had_any_pbf) && !pending.remote_chars.is_empty() {
        let fallback: Vec<(u32, char)> = pending
            .remote_chars
            .iter()
            .filter_map(|&code| {
                let already = builder
                    .atlas
                    .glyphs
                    .get(&pending.font)
                    .is_some_and(|map| map.contains_key(&code));
                (!already)
                    .then(|| char::from_u32(code).map(|ch| (code, ch)))
                    .flatten()
            })
            .collect();
        if !fallback.is_empty() {
            let family = ideograph_family.or(Some("sans-serif"));
            for (code, glyph) in
                crate::font::rasterize_canvas2d_chars_sdf(family, &pending.font, &fallback)
            {
                builder.pack_rasterized_codepoint(&pending.font, code, &glyph);
            }
        }
    }
    let _ = (glyphs_url, had_any_pbf);

    let mut fallback_used = false;
    if (pending.has_registered || cfg!(not(target_arch = "wasm32")))
        && ideograph_family.is_some()
        && !pending.remote_chars.is_empty()
    {
        let mut fs = global_font_system();
        for &code in &pending.remote_chars {
            let already_packed = builder
                .atlas
                .glyphs
                .get(&pending.font)
                .is_some_and(|map| map.contains_key(&code));
            if already_packed {
                continue;
            }
            let Some(ch) = char::from_u32(code) else {
                continue;
            };
            if let Some(glyph) = fs.rasterize_local_char(ideograph_family, &pending.font, ch) {
                builder.pack_rasterized_codepoint(&pending.font, code, &glyph);
                fallback_used = true;
            }
        }
    }
    fallback_used
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
        shape_and_pack_direction(
            &mut fs,
            font,
            ideograph_family,
            has_registered,
            label,
            false,
            builder,
        );
        if has_registered {
            shape_and_pack_direction(&mut fs, font, ideograph_family, true, label, true, builder);
        }
    }
}

fn shape_and_pack_direction(
    fs: &mut std::sync::MutexGuard<'_, crate::font::FontSystem>,
    font: &str,
    ideograph_family: Option<&str>,
    has_registered: bool,
    label: &str,
    vertical: bool,
    builder: &mut AtlasBuilder,
) {
    let shaped = if has_registered {
        let all_registered = label
            .chars()
            .filter(|ch| !ch.is_whitespace() && (!vertical || !ch.is_control()))
            .all(|ch| fs.resolve_face_for_char(font, ch, false).is_some());
        let target = if all_registered {
            Some((font.to_owned(), false))
        } else if ideograph_family.is_some() {
            Some((combined_stack(font, ideograph_family), true))
        } else {
            None
        };
        target.and_then(|(stack, allow_fallback)| {
            if vertical {
                fs.shape_text_vertical(&stack, label, allow_fallback)
            } else {
                fs.shape_text(&stack, label, allow_fallback)
            }
        })
    } else if !vertical && label.chars().any(is_complex_shaping_codepoint) {
        let stack = combined_stack(font, ideograph_family);
        fs.shape_text(&stack, label, true)
    } else {
        None
    };

    let Some(shaped) = shaped.filter(|s| !s.is_empty()) else {
        return;
    };
    for sg in &shaped {
        let key = sg.atlas_key();
        if !builder.atlas.face_glyphs.contains_key(&key) {
            if let Some(glyph) = fs.rasterize_face_glyph(sg.face_id, sg.glyph_id) {
                builder.pack_rasterized_face_glyph(key.clone(), &glyph);
            }
        }
        if !builder.atlas.face_glyphs.contains_key(&key) {
            return;
        }
    }
    let target = if vertical {
        &mut builder.atlas.vertical_shaped_runs
    } else {
        &mut builder.atlas.shaped_runs
    };
    target
        .entry(font.to_owned())
        .or_default()
        .insert(label.to_owned(), shaped);
}

fn combined_stack(font: &str, ideograph_family: Option<&str>) -> String {
    match ideograph_family {
        Some(family) if !family.is_empty() && !font.is_empty() => format!("{font},{family}"),
        Some(family) if !family.is_empty() => family.to_owned(),
        _ => font.to_owned(),
    }
}
