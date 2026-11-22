//! Zero-download browser `OffscreenCanvas` 2D local CJK / fallback SDF glyph rasteriser (`wasm32`)
//! with per-realm or shared glyph caching and grid-batched `getImageData` readbacks.

use std::{cell::RefCell, sync::Arc};

use rustc_hash::FxHashMap as HashMap;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{OffscreenCanvas, OffscreenCanvasRenderingContext2d};

use super::{
    coverage_to_sdf, RasterizedGlyph, MAX_GLYPH_DIM, PBF_BASELINE_TOP, SDF_BUFFER, SDF_EM_SIZE,
};
use crate::font::spec::{parse_font_stack_entry, FontStyle};

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
const MAX_CACHED_GLYPHS: usize = 2048;
#[cfg(not(all(target_arch = "wasm32", target_feature = "atomics")))]
const MAX_CACHED_GLYPHS: usize = 4096;
const BATCH_CELL_SIZE: usize = 48;
const BATCH_COLS: usize = 8;
const BATCH_CHUNK: usize = 64;

#[cfg(web_sys_unstable_apis)]
fn get_image_data(
    ctx: &OffscreenCanvasRenderingContext2d,
    width: i32,
    height: i32,
) -> Result<web_sys::ImageData, JsValue> {
    ctx.get_image_data(0, 0, width, height)
}

#[cfg(not(web_sys_unstable_apis))]
fn get_image_data(
    ctx: &OffscreenCanvasRenderingContext2d,
    width: i32,
    height: i32,
) -> Result<web_sys::ImageData, JsValue> {
    ctx.get_image_data(0.0, 0.0, f64::from(width), f64::from(height))
}

struct CanvasScratch {
    canvas: OffscreenCanvas,
    ctx: OffscreenCanvasRenderingContext2d,
}

#[derive(Default)]
struct GlyphCacheState {
    fonts: HashMap<String, HashMap<char, Option<Arc<RasterizedGlyph>>>>,
    total: usize,
}

impl GlyphCacheState {
    fn insert_batch(
        &mut self,
        css_font: &str,
        items: impl IntoIterator<Item = (char, Option<Arc<RasterizedGlyph>>)>,
    ) {
        for (ch, value) in items {
            if self.total >= MAX_CACHED_GLYPHS {
                self.fonts.clear();
                self.total = 0;
            }
            if !self.fonts.contains_key(css_font) {
                self.fonts.insert(css_font.to_owned(), HashMap::default());
            }
            if let Some(font_map) = self.fonts.get_mut(css_font) {
                if font_map.insert(ch, value).is_none() {
                    self.total += 1;
                }
            }
        }
    }
}

thread_local! {
    static CANVAS_SCRATCH: RefCell<Option<CanvasScratch>> = const { RefCell::new(None) };
    #[cfg(not(all(target_arch = "wasm32", target_feature = "atomics")))]
    static GLYPH_CACHE: RefCell<GlyphCacheState> = RefCell::new(GlyphCacheState::default());
}

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
static SHARED_GLYPH_CACHE: std::sync::LazyLock<std::sync::Mutex<GlyphCacheState>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(GlyphCacheState::default()));

fn with_glyph_cache<R>(f: impl FnOnce(&mut GlyphCacheState) -> R) -> R {
    #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
    {
        let mut guard = crate::platform::lock_unpoisoned(&SHARED_GLYPH_CACHE);
        f(&mut guard)
    }
    #[cfg(not(all(target_arch = "wasm32", target_feature = "atomics")))]
    {
        GLYPH_CACHE.with(|cache| f(&mut cache.borrow_mut()))
    }
}

fn store_cached(css_font: &str, ch: char, value: Option<Arc<RasterizedGlyph>>) {
    with_glyph_cache(|cache| cache.insert_batch(css_font, std::iter::once((ch, value))));
}

fn ensure_scratch(
    min_w: u32,
    min_h: u32,
) -> Option<(OffscreenCanvas, OffscreenCanvasRenderingContext2d)> {
    CANVAS_SCRATCH.with(|slot| {
        let mut borrow = slot.borrow_mut();
        if borrow.is_none() {
            let init_w = min_w.max(384);
            let init_h = min_h.max(384);
            let canvas = OffscreenCanvas::new(init_w, init_h).ok()?;
            let opts = js_sys::Object::new();
            let _ = js_sys::Reflect::set(
                &opts,
                &JsValue::from_str("willReadFrequently"),
                &JsValue::TRUE,
            );
            let ctx_obj = canvas
                .get_context_with_context_options("2d", &opts)
                .ok()
                .flatten()?;
            let ctx = ctx_obj
                .dyn_into::<OffscreenCanvasRenderingContext2d>()
                .ok()?;
            *borrow = Some(CanvasScratch { canvas, ctx });
        }
        let scratch = borrow.as_mut()?;
        if scratch.canvas.width() < min_w {
            scratch.canvas.set_width(min_w.next_power_of_two().max(64));
        }
        if scratch.canvas.height() < min_h {
            scratch.canvas.set_height(min_h.next_power_of_two().max(64));
        }
        Some((scratch.canvas.clone(), scratch.ctx.clone()))
    })
}

struct MeasuredChar {
    code: u32,
    ch: char,
    width: u32,
    height: u32,
    padded_w: usize,
    padded_h: usize,
    min_x: i32,
    min_y: i32,
    advance: u32,
}

/// Batch-rasterises `(code, ch)` pairs with a single `getImageData` readback per 64-glyph chunk
/// and caches results per worker realm so repeated tiles incur zero Canvas 2D overhead.
pub(crate) fn rasterize_canvas2d_chars_sdf(
    ideograph_family: Option<&str>,
    font_stack: &str,
    chars: &[(u32, char)],
) -> Vec<(u32, Arc<RasterizedGlyph>)> {
    if chars.is_empty() {
        return Vec::new();
    }
    let css_font = build_css_font(ideograph_family, font_stack);
    let mut out = Vec::with_capacity(chars.len());
    let mut uncached = Vec::new();
    with_glyph_cache(|cache| {
        let font_map = cache.fonts.get(css_font.as_str());
        for &(code, ch) in chars {
            if ch.is_control() || matches!(ch, '\u{200B}'..='\u{200D}' | '\u{FEFF}') {
                continue;
            }
            match font_map.and_then(|map| map.get(&ch)) {
                Some(Some(glyph)) => out.push((code, Arc::clone(glyph))),
                Some(None) => {}
                None => uncached.push((code, ch)),
            }
        }
    });
    if uncached.is_empty() {
        return out;
    }

    let Some((_, ctx)) = ensure_scratch(384, 384) else {
        return out;
    };
    ctx.set_font(&css_font);
    ctx.set_text_baseline("alphabetic");
    ctx.set_text_align("left");

    let mut batchable = Vec::with_capacity(uncached.len());
    let mut measured_cache = Vec::new();
    for (code, ch) in uncached {
        match measure_one(&ctx, code, ch) {
            MeasureOutcome::Empty(glyph) => {
                let arc = Arc::new(glyph);
                measured_cache.push((ch, Some(Arc::clone(&arc))));
                out.push((code, arc));
            }
            MeasureOutcome::Skip => measured_cache.push((ch, None)),
            MeasureOutcome::Ink(m) => {
                if m.padded_w <= BATCH_CELL_SIZE && m.padded_h <= BATCH_CELL_SIZE {
                    batchable.push(m);
                } else {
                    let g = rasterize_single_measured(&css_font, &m).map(Arc::new);
                    store_cached(&css_font, m.ch, g.clone());
                    if let Some(arc) = g {
                        out.push((m.code, arc));
                    }
                }
            }
        }
    }
    if !measured_cache.is_empty() {
        with_glyph_cache(|cache| cache.insert_batch(&css_font, measured_cache));
    }

    for chunk in batchable.chunks(BATCH_CHUNK) {
        rasterize_grid_chunk(&css_font, chunk, &mut out);
    }
    out
}

/// Rasterises a single `ch` at `24px` (used by `FontSystem::rasterize_local_char`).
pub(in crate::font) fn rasterize_canvas2d_char_sdf(
    ideograph_family: Option<&str>,
    font_stack: &str,
    ch: char,
) -> Option<RasterizedGlyph> {
    let mut batch =
        rasterize_canvas2d_chars_sdf(ideograph_family, font_stack, &[(u32::from(ch), ch)]);
    batch.pop().map(|(_, arc)| (*arc).clone())
}

enum MeasureOutcome {
    Empty(RasterizedGlyph),
    Ink(MeasuredChar),
    Skip,
}

fn measure_one(ctx: &OffscreenCanvasRenderingContext2d, code: u32, ch: char) -> MeasureOutcome {
    let mut buf = [0u8; 4];
    let text = ch.encode_utf8(&mut buf);
    let Ok(metrics) = ctx.measure_text(text) else {
        return MeasureOutcome::Skip;
    };
    let advance = metrics.width().round().max(0.0) as u32;
    if ch.is_whitespace() {
        return MeasureOutcome::Empty(RasterizedGlyph {
            width: 0,
            height: 0,
            left: 0,
            top: PBF_BASELINE_TOP,
            advance,
            bitmap: Vec::new(),
        });
    }

    let mut min_x = (-metrics.actual_bounding_box_left()).floor() as i32;
    let mut max_x = metrics.actual_bounding_box_right().ceil() as i32;
    let mut min_y = (-metrics.actual_bounding_box_ascent()).floor() as i32;
    let mut max_y = metrics.actual_bounding_box_descent().ceil() as i32;
    if max_x <= min_x || max_y <= min_y {
        if advance == 0 {
            return MeasureOutcome::Skip;
        }
        min_x = 0;
        max_x = (advance as i32).max(SDF_EM_SIZE as i32);
        min_y = -21;
        max_y = 5;
    }

    let raw_w = (max_x - min_x).clamp(0, MAX_GLYPH_DIM);
    let raw_h = (max_y - min_y).clamp(0, MAX_GLYPH_DIM);
    if raw_w == 0 || raw_h == 0 {
        return MeasureOutcome::Empty(RasterizedGlyph {
            width: 0,
            height: 0,
            left: 0,
            top: PBF_BASELINE_TOP,
            advance,
            bitmap: Vec::new(),
        });
    }
    let width = raw_w as u32;
    let height = raw_h as u32;
    let pad = SDF_BUFFER as usize;
    MeasureOutcome::Ink(MeasuredChar {
        code,
        ch,
        width,
        height,
        padded_w: width as usize + pad * 2,
        padded_h: height as usize + pad * 2,
        min_x,
        min_y,
        advance,
    })
}

fn rasterize_grid_chunk(
    css_font: &str,
    chunk: &[MeasuredChar],
    out: &mut Vec<(u32, Arc<RasterizedGlyph>)>,
) {
    let cols = chunk.len().min(BATCH_COLS);
    let rows = chunk.len().div_ceil(cols);
    let grid_w = cols * BATCH_CELL_SIZE;
    let grid_h = rows * BATCH_CELL_SIZE;
    let Some((_, ctx)) = ensure_scratch(grid_w as u32, grid_h as u32) else {
        return;
    };
    ctx.clear_rect(0.0, 0.0, grid_w as f64, grid_h as f64);
    ctx.set_font(css_font);
    ctx.set_text_baseline("alphabetic");
    ctx.set_text_align("left");
    let _ = js_sys::Reflect::set(
        ctx.as_ref(),
        &JsValue::from_str("fillStyle"),
        &JsValue::from_str("#ffffff"),
    );

    let mut buf = [0u8; 4];
    for (idx, m) in chunk.iter().enumerate() {
        let col = idx % cols;
        let row = idx / cols;
        let cell_x = (col * BATCH_CELL_SIZE) as f64;
        let cell_y = (row * BATCH_CELL_SIZE) as f64;
        ctx.save();
        ctx.begin_path();
        ctx.rect(cell_x, cell_y, m.padded_w as f64, m.padded_h as f64);
        ctx.clip();
        let draw_x = cell_x + f64::from(SDF_BUFFER) - f64::from(m.min_x);
        let draw_y = cell_y + f64::from(SDF_BUFFER) - f64::from(m.min_y);
        let _ = ctx.fill_text(m.ch.encode_utf8(&mut buf), draw_x, draw_y);
        ctx.restore();
    }

    let Ok(image_data) = get_image_data(&ctx, grid_w as i32, grid_h as i32) else {
        return;
    };
    let rgba = image_data.data();
    if rgba.len() < grid_w * grid_h * 4 {
        return;
    }

    let mut coverage = Vec::new();
    let mut chunk_cache = Vec::with_capacity(chunk.len());
    for (idx, m) in chunk.iter().enumerate() {
        let col = idx % cols;
        let row = idx / cols;
        let base_x = col * BATCH_CELL_SIZE;
        let base_y = row * BATCH_CELL_SIZE;
        coverage.clear();
        coverage.reserve(m.padded_w * m.padded_h);
        let mut has_ink = false;
        for py in 0..m.padded_h {
            let row_start = ((base_y + py) * grid_w + base_x) * 4;
            for px in 0..m.padded_w {
                let alpha = rgba[row_start + px * 4 + 3];
                if alpha > 0 {
                    has_ink = true;
                }
                coverage.push(f32::from(alpha) / 255.0);
            }
        }
        let built = has_ink.then(|| {
            Arc::new(RasterizedGlyph {
                width: m.width,
                height: m.height,
                left: m.min_x,
                top: PBF_BASELINE_TOP - m.min_y,
                advance: m.advance,
                bitmap: coverage_to_sdf(&coverage, m.padded_w, m.padded_h),
            })
        });
        chunk_cache.push((m.ch, built.clone()));
        if let Some(arc) = built {
            out.push((m.code, arc));
        }
    }
    with_glyph_cache(|cache| cache.insert_batch(css_font, chunk_cache));
}

fn rasterize_single_measured(css_font: &str, m: &MeasuredChar) -> Option<RasterizedGlyph> {
    let (_, ctx) = ensure_scratch(m.padded_w as u32, m.padded_h as u32)?;
    ctx.clear_rect(0.0, 0.0, m.padded_w as f64, m.padded_h as f64);
    ctx.set_font(css_font);
    ctx.set_text_baseline("alphabetic");
    ctx.set_text_align("left");
    let _ = js_sys::Reflect::set(
        ctx.as_ref(),
        &JsValue::from_str("fillStyle"),
        &JsValue::from_str("#ffffff"),
    );
    let mut buf = [0u8; 4];
    let draw_x = f64::from(SDF_BUFFER) - f64::from(m.min_x);
    let draw_y = f64::from(SDF_BUFFER) - f64::from(m.min_y);
    let _ = ctx.fill_text(m.ch.encode_utf8(&mut buf), draw_x, draw_y);
    let image_data = get_image_data(&ctx, m.padded_w as i32, m.padded_h as i32).ok()?;
    let rgba = image_data.data();
    let total = m.padded_w * m.padded_h;
    if rgba.len() < total * 4 {
        return None;
    }
    let mut has_ink = false;
    let mut coverage = Vec::with_capacity(total);
    for idx in 0..total {
        let alpha = rgba[idx * 4 + 3];
        if alpha > 0 {
            has_ink = true;
        }
        coverage.push(f32::from(alpha) / 255.0);
    }
    has_ink.then(|| RasterizedGlyph {
        width: m.width,
        height: m.height,
        left: m.min_x,
        top: PBF_BASELINE_TOP - m.min_y,
        advance: m.advance,
        bitmap: coverage_to_sdf(&coverage, m.padded_w, m.padded_h),
    })
}

fn build_css_font(ideograph_family: Option<&str>, font_stack: &str) -> String {
    let first = font_stack.split(',').next().unwrap_or(font_stack);
    let (_, weight, style) = parse_font_stack_entry(first);
    let posture = match style {
        FontStyle::Normal => "normal",
        FontStyle::Italic => "italic",
        FontStyle::Oblique => "oblique",
    };

    let mut families: Vec<String> = Vec::new();
    if let Some(ideo) = ideograph_family {
        for raw in ideo.split(',') {
            push_css_family(&mut families, raw);
        }
    }
    for raw in font_stack.split(',') {
        let (clean, _, _) = parse_font_stack_entry(raw);
        push_css_family(&mut families, &clean);
    }
    push_css_family(&mut families, "sans-serif");

    format!(
        "{posture} {} {}px {}",
        weight.0,
        SDF_EM_SIZE as u32,
        families.join(", ")
    )
}

fn push_css_family(out: &mut Vec<String>, raw: &str) {
    let trimmed = raw.trim().trim_matches(['"', '\'']);
    if trimmed.is_empty() {
        return;
    }
    let formatted = if matches!(
        trimmed.to_ascii_lowercase().as_str(),
        "sans-serif" | "serif" | "monospace" | "system-ui" | "cursive" | "fantasy"
    ) {
        trimmed.to_ascii_lowercase()
    } else {
        format!("\"{}\"", trimmed.replace('"', ""))
    };
    if !out
        .iter()
        .any(|existing| existing.eq_ignore_ascii_case(&formatted))
    {
        out.push(formatted);
    }
}
