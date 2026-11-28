//! Signed Distance Field (SDF) glyph rasterisation matching MapLibre's 24px PBF glyph metrics.
//!
//! Uses `skrifa` to extract unhinted outlines at `24.0` px, `ab_glyph_rasterizer` for subpixel
//! coverage, and the 2D Felzenszwalb & Huttenlocher Euclidean distance transform (`TinySDF`
//! subpixel boundary seeding) with `buffer = 3`, `radius = 8.0`, and `cutoff = 0.25` (`0.75`
//! inside-edge threshold in `sdf.fragment.wgsl`).

#[cfg(feature = "font-shaping")]
use ab_glyph_rasterizer::{point, Point, Rasterizer};
#[cfg(feature = "font-shaping")]
use skrifa::{
    instance::{LocationRef, Size},
    outline::{DrawSettings, OutlinePen},
    FontRef, GlyphId, MetadataProvider,
};

#[cfg(target_arch = "wasm32")]
pub(super) mod canvas2d;
#[cfg(target_arch = "wasm32")]
pub(crate) use canvas2d::rasterize_canvas2d_chars_sdf;

/// Nominal em size in pixels used by MapLibre SDF glyph atlases.
pub const SDF_EM_SIZE: f32 = 24.0;
/// Transparent SDF padding border around the unpadded glyph box on each side, in pixels.
pub const SDF_BUFFER: u32 = 3;
/// Distance field spread in pixels from full interior (`255`) to full exterior (`0`).
pub const SDF_RADIUS: f32 = 8.0;
/// Fraction of [`SDF_RADIUS`] reserved inside the glyph contour (`1.0 - 0.25 = 0.75` edge).
pub const SDF_CUTOFF: f32 = 0.25;
/// Baseline reference offset matching MapLibre GL JS `BASELINE_OFFSET = -27` (`top = -27 - min_y`).
#[cfg(any(feature = "font-shaping", target_arch = "wasm32"))]
pub(super) const PBF_BASELINE_TOP: i32 = -27;
/// Maximum unpadded glyph dimension rasterised to guard against degenerate outlines.
#[cfg(any(feature = "font-shaping", target_arch = "wasm32"))]
pub(super) const MAX_GLYPH_DIM: i32 = 512;
/// Sentinel squared distance for the Felzenszwalb & Huttenlocher distance transform (`f64` exact).
#[cfg(any(feature = "font-shaping", target_arch = "wasm32"))]
const EDT_INF: f64 = 1.0e12;

/// One glyph rasterised at 24px nominal em-size in MapLibre's SDF conventions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RasterizedGlyph {
    /// Unpadded glyph width in pixels (bitmap width is `width + 6` when non-empty).
    pub width: u32,
    /// Unpadded glyph height in pixels (bitmap height is `height + 6` when non-empty).
    pub height: u32,
    /// Horizontal bearing from the pen origin to the unpadded left edge, in pixels.
    pub left: i32,
    /// Vertical bearing in MapLibre PBF convention (`-27 - min_y`, or `-27` when empty).
    pub top: i32,
    /// Horizontal advance in 24px-em pixels.
    pub advance: u32,
    /// Row-major 8-bit SDF bitmap of length `(width + 6) * (height + 6)`, or empty for whitespace.
    pub bitmap: Vec<u8>,
}

impl RasterizedGlyph {
    /// Converts this rasterised glyph into a decoded PBF `Glyph` record.
    pub fn to_pbf_glyph(&self, id: u32) -> crate::font::glyphs::Glyph {
        crate::font::glyphs::Glyph {
            id,
            bitmap: (!self.bitmap.is_empty()).then(|| self.bitmap.clone()),
            width: self.width,
            height: self.height,
            left: self.left,
            top: self.top,
            advance: self.advance,
        }
    }
}

/// Rasterises the nominal `cmap` glyph of `ch` from `font_bytes` (face `face_index`).
///
/// Returns `None` if `font_bytes` cannot be parsed or the font lacks a mapping for `ch`
/// (`.notdef` glyph `0` is rejected).
pub fn rasterize_char_sdf(font_bytes: &[u8], face_index: u32, ch: char) -> Option<RasterizedGlyph> {
    #[cfg(feature = "font-shaping")]
    {
        let font = FontRef::from_index(font_bytes, face_index).ok()?;
        let glyph_id = font.charmap().map(ch)?;
        if glyph_id == GlyphId::NOTDEF {
            return None;
        }
        rasterize_glyph_from_ref(&font, glyph_id)
    }
    #[cfg(not(feature = "font-shaping"))]
    {
        let _ = (font_bytes, face_index, ch);
        None
    }
}

/// Rasterises `glyph_id` (including unencoded `GSUB` ligature/conjunct glyphs) from `font_bytes`.
pub fn rasterize_glyph_sdf(
    font_bytes: &[u8],
    face_index: u32,
    glyph_id: u32,
) -> Option<RasterizedGlyph> {
    #[cfg(feature = "font-shaping")]
    {
        let font = FontRef::from_index(font_bytes, face_index).ok()?;
        rasterize_glyph_from_ref(&font, GlyphId::new(glyph_id))
    }
    #[cfg(not(feature = "font-shaping"))]
    {
        let _ = (font_bytes, face_index, glyph_id);
        None
    }
}

#[cfg(feature = "font-shaping")]
pub(crate) fn rasterize_glyph_from_ref(
    font: &FontRef<'_>,
    glyph_id: GlyphId,
) -> Option<RasterizedGlyph> {
    let size = Size::new(SDF_EM_SIZE);
    let loc = LocationRef::default();
    let advance = font
        .glyph_metrics(size, loc)
        .advance_width(glyph_id)
        .unwrap_or(0.0)
        .round()
        .max(0.0) as u32;

    let mut pen = RecordingPen::default();
    if let Some(outline) = font.outline_glyphs().get(glyph_id) {
        let settings = DrawSettings::unhinted(size, loc);
        if outline.draw(settings, &mut pen).is_err() {
            return None;
        }
        pen.finish_open_subpath();
    }

    Some(pen.rasterize(advance))
}

#[cfg(feature = "font-shaping")]
fn empty_rasterized_glyph(advance: u32) -> RasterizedGlyph {
    RasterizedGlyph {
        width: 0,
        height: 0,
        left: 0,
        top: PBF_BASELINE_TOP,
        advance,
        bitmap: Vec::new(),
    }
}

#[cfg(feature = "font-shaping")]
fn rasterize_outline_commands(
    commands: &[Segment],
    min_x: i32,
    min_y: i32,
    width: u32,
    height: u32,
    advance: u32,
) -> RasterizedGlyph {
    let pad = SDF_BUFFER as usize;
    let padded_w = width as usize + pad * 2;
    let padded_h = height as usize + pad * 2;
    let shift_x = pad as f32 - min_x as f32;
    let shift_y = pad as f32 - min_y as f32;

    let mut rasterizer = Rasterizer::new(padded_w, padded_h);
    for cmd in commands {
        let map_pt = |p: Point| point(p.x + shift_x, p.y + shift_y);
        match *cmd {
            Segment::Line(p0, p1) => rasterizer.draw_line(map_pt(p0), map_pt(p1)),
            Segment::Quad(p0, p1, p2) => {
                rasterizer.draw_quad(map_pt(p0), map_pt(p1), map_pt(p2));
            }
            Segment::Cubic(p0, p1, p2, p3) => {
                rasterizer.draw_cubic(map_pt(p0), map_pt(p1), map_pt(p2), map_pt(p3));
            }
        }
    }

    let mut coverage = vec![0.0_f32; padded_w * padded_h];
    rasterizer.for_each_pixel(|idx, alpha| {
        if let Some(slot) = coverage.get_mut(idx) {
            *slot = alpha.clamp(0.0, 1.0);
        }
    });

    RasterizedGlyph {
        width,
        height,
        left: min_x,
        top: PBF_BASELINE_TOP.saturating_sub(min_y),
        advance,
        bitmap: coverage_to_sdf(&coverage, padded_w, padded_h),
    }
}

#[cfg(feature = "font-shaping")]
#[derive(Clone, Copy)]
enum Segment {
    Line(Point, Point),
    Quad(Point, Point, Point),
    Cubic(Point, Point, Point, Point),
}

#[cfg(feature = "font-shaping")]
struct RecordingPen {
    commands: Vec<Segment>,
    start: Point,
    current: Point,
    open: bool,
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
}

#[cfg(feature = "font-shaping")]
impl Default for RecordingPen {
    fn default() -> Self {
        Self {
            commands: Vec::new(),
            start: point(0.0, 0.0),
            current: point(0.0, 0.0),
            open: false,
            min_x: f32::INFINITY,
            min_y: f32::INFINITY,
            max_x: f32::NEG_INFINITY,
            max_y: f32::NEG_INFINITY,
        }
    }
}

#[cfg(feature = "font-shaping")]
impl RecordingPen {
    fn include(&mut self, p: Point) {
        self.min_x = self.min_x.min(p.x);
        self.min_y = self.min_y.min(p.y);
        self.max_x = self.max_x.max(p.x);
        self.max_y = self.max_y.max(p.y);
    }

    fn finish_open_subpath(&mut self) {
        if self.open {
            if (self.current.x - self.start.x).abs() > 1e-5
                || (self.current.y - self.start.y).abs() > 1e-5
            {
                self.commands.push(Segment::Line(self.current, self.start));
            }
            self.current = self.start;
            self.open = false;
        }
    }

    fn rasterize(&self, advance: u32) -> RasterizedGlyph {
        if self.commands.is_empty()
            || !self.min_x.is_finite()
            || !self.min_y.is_finite()
            || !self.max_x.is_finite()
            || !self.max_y.is_finite()
            || self.max_x <= self.min_x
            || self.max_y <= self.min_y
        {
            return empty_rasterized_glyph(advance);
        }

        const MAX_OUTLINE_COORD: f32 = 4096.0;
        let clamp_coord = |v: f32| v.clamp(-MAX_OUTLINE_COORD, MAX_OUTLINE_COORD);
        let (min_x, min_y) = (
            clamp_coord(self.min_x).floor() as i32,
            clamp_coord(self.min_y).floor() as i32,
        );
        let (max_x, max_y) = (
            clamp_coord(self.max_x).ceil() as i32,
            clamp_coord(self.max_y).ceil() as i32,
        );
        let raw_w = max_x.saturating_sub(min_x).clamp(0, MAX_GLYPH_DIM);
        let raw_h = max_y.saturating_sub(min_y).clamp(0, MAX_GLYPH_DIM);
        if raw_w == 0 || raw_h == 0 {
            return empty_rasterized_glyph(advance);
        }

        rasterize_outline_commands(
            &self.commands,
            min_x,
            min_y,
            raw_w as u32,
            raw_h as u32,
            advance,
        )
    }
}

#[cfg(feature = "font-shaping")]
impl OutlinePen for RecordingPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.finish_open_subpath();
        let p = point(x, -y);
        self.start = p;
        self.current = p;
        self.open = true;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p = point(x, -y);
        self.include(self.current);
        self.include(p);
        self.commands.push(Segment::Line(self.current, p));
        self.current = p;
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        let c0 = point(cx0, -cy0);
        let p = point(x, -y);
        self.include(self.current);
        self.include(c0);
        self.include(p);
        self.commands.push(Segment::Quad(self.current, c0, p));
        self.current = p;
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let c0 = point(cx0, -cy0);
        let c1 = point(cx1, -cy1);
        let p = point(x, -y);
        self.include(self.current);
        self.include(c0);
        self.include(c1);
        self.include(p);
        self.commands.push(Segment::Cubic(self.current, c0, c1, p));
        self.current = p;
    }

    fn close(&mut self) {
        self.finish_open_subpath();
    }
}

/// Converts subpixel `[0, 1]` coverage values into an 8-bit TinySDF bitmap.
#[cfg(any(feature = "font-shaping", target_arch = "wasm32"))]
pub(super) fn coverage_to_sdf(coverage: &[f32], width: usize, height: usize) -> Vec<u8> {
    let total = width * height;
    let mut grid_outer = vec![EDT_INF; total];
    let mut grid_inner = vec![0.0_f64; total];
    for ((&alpha, outer), inner) in coverage
        .iter()
        .zip(grid_outer.iter_mut())
        .zip(grid_inner.iter_mut())
    {
        // Quantize to 8-bit coverage (matching Canvas 2D + TinySDF) so `f32` scanline prefix-sum
        // rounding residuals (`~1e-7` near `0.0` or `0.9999999` near `1.0`) never seed false
        // contour boundaries across empty padding or solid stroke interiors.
        let quantized = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
        if quantized == 0 {
            *outer = EDT_INF;
            *inner = 0.0;
        } else if quantized == u8::MAX {
            *outer = 0.0;
            *inner = EDT_INF;
        } else {
            let a = f64::from(quantized) / 255.0;
            let delta = 0.5 - a;
            *outer = if delta > 0.0 { delta * delta } else { 0.0 };
            *inner = if delta < 0.0 { delta * delta } else { 0.0 };
        }
    }

    edt_2d(&mut grid_outer, width, height);
    edt_2d(&mut grid_inner, width, height);

    let edge = f64::from(1.0 - SDF_CUTOFF);
    let radius = f64::from(SDF_RADIUS);
    grid_outer
        .iter()
        .zip(grid_inner.iter())
        .map(|(&outer, &inner)| {
            let dist = outer.sqrt() - inner.sqrt();
            ((edge - dist / radius) * 255.0).round().clamp(0.0, 255.0) as u8
        })
        .collect()
}

/// In-place 2D squared Euclidean distance transform (Felzenszwalb & Huttenlocher 2012).
#[cfg(any(feature = "font-shaping", target_arch = "wasm32"))]
#[expect(
    clippy::indexing_slicing,
    reason = "2D EDT scratch buffers use max(width, height) and guard checks grid.len() >= width * height"
)]
fn edt_2d(grid: &mut [f64], width: usize, height: usize) {
    let max_len = width.max(height);
    if max_len == 0 || grid.len() < width * height {
        return;
    }
    let mut f = vec![0.0_f64; max_len];
    let mut d = vec![0.0_f64; max_len];
    let mut v = vec![0usize; max_len];
    let mut z = vec![0.0_f64; max_len + 1];

    for y in 0..height {
        let row = y * width;
        f[..width].copy_from_slice(&grid[row..row + width]);
        edt_1d(
            &f[..width],
            &mut d[..width],
            &mut v[..width],
            &mut z[..=width],
        );
        grid[row..row + width].copy_from_slice(&d[..width]);
    }
    for x in 0..width {
        for y in 0..height {
            f[y] = grid[y * width + x];
        }
        edt_1d(
            &f[..height],
            &mut d[..height],
            &mut v[..height],
            &mut z[..=height],
        );
        for y in 0..height {
            grid[y * width + x] = d[y];
        }
    }
}

#[cfg(any(feature = "font-shaping", target_arch = "wasm32"))]
#[expect(
    clippy::indexing_slicing,
    reason = "1D EDT hull stack enforces 0 <= k <= q < n with output/scratch lengths checked up front"
)]
fn edt_1d(f: &[f64], d: &mut [f64], v: &mut [usize], z: &mut [f64]) {
    let n = f.len();
    if n == 0 || d.len() < n || v.len() < n || z.len() <= n {
        return;
    }
    v[0] = 0;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    let mut k = 0usize;
    for q in 1..n {
        let qf = q as f64;
        let s = loop {
            let vk = v[k];
            let vkf = vk as f64;
            let intersect = ((f[q] + qf * qf) - (f[vk] + vkf * vkf)) / (2.0 * (qf - vkf));
            if intersect > z[k] || k == 0 {
                break intersect;
            }
            k -= 1;
        };
        k += 1;
        v[k] = q;
        z[k] = s.max(z[k - 1]);
        z[k + 1] = f64::INFINITY;
    }

    k = 0;
    for (q, out) in d.iter_mut().enumerate() {
        let qf = q as f64;
        while z[k + 1] < qf {
            k += 1;
        }
        let delta = qf - v[k] as f64;
        *out = delta * delta + f[v[k]];
    }
}
