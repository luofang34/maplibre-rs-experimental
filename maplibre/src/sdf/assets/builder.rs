//! Incremental shelf atlas packer for symbol glyphs and sprite icons.
use std::sync::Arc;

use prost::Message;
use rustc_hash::FxHashSet;

use super::{AtlasEntry, SymbolAtlas};
use crate::sdf::glyphs;

const INITIAL_ATLAS_WIDTH: u32 = 512;
#[cfg(target_arch = "wasm32")]
const MAX_PIXELS_OVERALLOC: usize = 128 * 1024;
#[cfg(not(target_arch = "wasm32"))]
const MAX_PIXELS_OVERALLOC: usize = 512 * 1024;

/// Incremental shelf packer with a transparent border around every image.
pub(in crate::sdf) struct AtlasBuilder {
    pub(in crate::sdf) atlas: SymbolAtlas,
    cursor: [u32; 2],
    row_height: u32,
}

impl AtlasBuilder {
    pub(in crate::sdf) fn new() -> Self {
        Self {
            atlas: SymbolAtlas {
                size: [INITIAL_ATLAS_WIDTH, 1],
                ..Default::default()
            },
            cursor: [1, 1],
            row_height: 0,
        }
    }

    fn widen_to_1024(&mut self) {
        let [old_w, old_h] = self.atlas.size;
        if old_w >= 1024 {
            return;
        }
        let new_w = 1024_u32;
        let mut widened = vec![0_u8; (new_w * old_h * 4) as usize];
        let old_row_bytes = (old_w * 4) as usize;
        let new_row_bytes = (new_w * 4) as usize;
        for row in 0..old_h as usize {
            let src = row * old_row_bytes;
            let dst = row * new_row_bytes;
            if let (Some(src_slice), Some(dst_slice)) = (
                self.atlas.pixels.get(src..src + old_row_bytes),
                widened.get_mut(dst..dst + old_row_bytes),
            ) {
                dst_slice.copy_from_slice(src_slice);
            }
        }
        self.atlas.pixels = widened;
        self.atlas.size[0] = new_w;
    }

    fn allocate_rect(&mut self, width: u32, height: u32) -> Option<([u32; 2], u32)> {
        if width > 1022 || height > 4094 {
            return None;
        }
        if width > self.atlas.size[0] - 2 {
            self.widen_to_1024();
        }
        let [mut atlas_width, _] = self.atlas.size;
        if self.cursor[0] + width + 1 > atlas_width {
            let next_y = self.cursor[1] + self.row_height + 2;
            if next_y + height + 1 > 4096 && atlas_width < 1024 {
                self.widen_to_1024();
                atlas_width = self.atlas.size[0];
            }
            if self.cursor[0] + width + 1 > atlas_width {
                self.cursor = [1, self.cursor[1] + self.row_height + 2];
                self.row_height = 0;
            }
        }
        let [x, y] = self.cursor;
        if y + height + 1 > 4096 {
            return None;
        }
        let atlas_height = self.atlas.size[1].max(y + height + 1);
        let needed = (atlas_height * atlas_width * 4) as usize;
        if needed > self.atlas.pixels.capacity() {
            let target = needed
                .saturating_mul(2)
                .min(needed.saturating_add(MAX_PIXELS_OVERALLOC));
            self.atlas
                .pixels
                .reserve(target.saturating_sub(self.atlas.pixels.len()));
        }
        self.atlas.pixels.resize(needed, 0);
        self.cursor[0] += width + 2;
        self.row_height = self.row_height.max(height);
        self.atlas.size[1] = atlas_height;
        Some(([x, y], atlas_width))
    }

    pub(in crate::sdf) fn pack(
        &mut self,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> Option<[u32; 4]> {
        if width > 1022 || height > 4094 || pixels.len() != (width as usize) * (height as usize) * 4
        {
            return None;
        }
        let ([x, y], atlas_width) = self.allocate_rect(width, height)?;
        let row_bytes = (width * 4) as usize;
        for row in 0..height {
            let start = (((y + row) * atlas_width + x) * 4) as usize;
            let source = (row * width * 4) as usize;
            let (Some(dst), Some(src)) = (
                self.atlas.pixels.get_mut(start..start + row_bytes),
                pixels.get(source..source + row_bytes),
            ) else {
                return None;
            };
            dst.copy_from_slice(src);
        }
        Some([x, y, width, height])
    }

    /// Writes single-channel SDF `alpha` bytes directly into RGBA atlas memory without a temporary `Vec<u8>`.
    pub(in crate::sdf::assets) fn pack_alpha_sdf(
        &mut self,
        width: u32,
        height: u32,
        alpha: &[u8],
    ) -> Option<[u32; 4]> {
        if width > 1022 || height > 4094 || alpha.len() != (width as usize) * (height as usize) {
            return None;
        }
        let ([x, y], atlas_width) = self.allocate_rect(width, height)?;
        let row_bytes = (width * 4) as usize;
        for row in 0..height {
            let start = (((y + row) * atlas_width + x) * 4) as usize;
            let src_start = (row * width) as usize;
            let (Some(src), Some(dst)) = (
                alpha.get(src_start..src_start + width as usize),
                self.atlas.pixels.get_mut(start..start + row_bytes),
            ) else {
                return None;
            };
            for (&a, px) in src.iter().zip(dst.as_chunks_mut::<4>().0) {
                *px = [a, a, a, 255];
            }
        }
        Some([x, y, width, height])
    }

    /// Copies a sub-rectangle directly from `image` into the atlas, optionally premultiplying RGB by alpha.
    pub(in crate::sdf::assets) fn pack_cropped_rgba(
        &mut self,
        image: &image::RgbaImage,
        [sx, sy, width, height]: [u32; 4],
        premul: bool,
    ) -> Option<[u32; 4]> {
        if width == 0
            || height == 0
            || sx.saturating_add(width) > image.width()
            || sy.saturating_add(height) > image.height()
        {
            return None;
        }
        let ([x, y], atlas_width) = self.allocate_rect(width, height)?;
        let img_stride = (image.width() * 4) as usize;
        let raw = image.as_raw();
        let row_bytes = (width * 4) as usize;
        for row in 0..height {
            let dst_start = (((y + row) * atlas_width + x) * 4) as usize;
            let src_start = (sy + row) as usize * img_stride + (sx * 4) as usize;
            let (Some(src), Some(dst)) = (
                raw.get(src_start..src_start + row_bytes),
                self.atlas.pixels.get_mut(dst_start..dst_start + row_bytes),
            ) else {
                return None;
            };
            if premul {
                for (s, d) in src
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(dst.as_chunks_mut::<4>().0)
                {
                    let alpha = u32::from(s[3]);
                    *d = [
                        ((u32::from(s[0]) * alpha + 127) / 255) as u8,
                        ((u32::from(s[1]) * alpha + 127) / 255) as u8,
                        ((u32::from(s[2]) * alpha + 127) / 255) as u8,
                        s[3],
                    ];
                }
            } else {
                dst.copy_from_slice(src);
            }
        }
        Some([x, y, width, height])
    }

    pub(in crate::sdf) fn glyph_range(
        &mut self,
        font: &str,
        bytes: &[u8],
    ) -> Result<(), prost::DecodeError> {
        self.glyph_subset(font, &glyphs::Glyphs::decode(bytes)?, None);
        Ok(())
    }

    pub(in crate::sdf::assets) fn glyph_subset(
        &mut self,
        font: &str,
        data: &glyphs::Glyphs,
        wanted: Option<&FxHashSet<u32>>,
    ) {
        for glyph in data.stacks.iter().flat_map(|stack| &stack.glyphs) {
            if wanted.is_some_and(|wanted| !wanted.contains(&glyph.id)) {
                continue;
            }
            if glyph.width > 1016 || glyph.height > 4088 {
                continue;
            }
            let rect =
                if let Some(bitmap) = glyph.bitmap.as_ref().filter(|bitmap| !bitmap.is_empty()) {
                    let Some(rect) = self.pack_alpha_sdf(glyph.width + 6, glyph.height + 6, bitmap)
                    else {
                        continue;
                    };
                    rect
                } else {
                    [0; 4]
                };
            self.atlas
                .glyphs
                .entry(font.to_string())
                .or_default()
                .insert(
                    glyph.id,
                    AtlasEntry {
                        rect,
                        metrics: [
                            glyph.left as f32 - 3.0,
                            glyph.top as f32 + 3.0,
                            glyph.advance as f32,
                            1.0,
                        ],
                        kind: 0,
                        ..Default::default()
                    },
                );
        }
    }

    pub(in crate::sdf::assets) fn pack_rasterized_codepoint(
        &mut self,
        font: &str,
        id: u32,
        glyph: &crate::font::RasterizedGlyph,
    ) {
        if let Some(entry) = self.entry_for_rasterized(glyph) {
            self.atlas
                .glyphs
                .entry(font.to_string())
                .or_default()
                .insert(id, entry);
        }
    }

    pub(in crate::sdf::assets) fn pack_rasterized_face_glyph(
        &mut self,
        key: String,
        glyph: &crate::font::RasterizedGlyph,
    ) {
        if self.atlas.face_glyphs.contains_key(&key) {
            return;
        }
        if let Some(entry) = self.entry_for_rasterized(glyph) {
            self.atlas.face_glyphs.insert(key, entry);
        }
    }

    fn entry_for_rasterized(&mut self, glyph: &crate::font::RasterizedGlyph) -> Option<AtlasEntry> {
        if glyph.width > 1016 || glyph.height > 4088 {
            return None;
        }
        let rect = if !glyph.bitmap.is_empty() {
            self.pack_alpha_sdf(glyph.width + 6, glyph.height + 6, &glyph.bitmap)?
        } else {
            [0; 4]
        };
        Some(AtlasEntry {
            rect,
            metrics: [
                glyph.left as f32 - 3.0,
                glyph.top as f32 + 3.0,
                glyph.advance as f32,
                1.0,
            ],
            kind: 0,
            ..Default::default()
        })
    }

    pub(in crate::sdf) fn finish(mut self) -> Arc<SymbolAtlas> {
        self.atlas.size[1] = self.atlas.size[1].max(1);
        if self.cursor[1] == 1 {
            let trimmed_w = self.cursor[0]
                .max(4)
                .next_multiple_of(4)
                .min(self.atlas.size[0]);
            if trimmed_w < self.atlas.size[0] {
                let old_w = self.atlas.size[0] as usize;
                let h = self.atlas.size[1] as usize;
                let new_w = trimmed_w as usize;
                let mut compact = vec![0_u8; new_w * h * 4];
                for row in 0..h {
                    let src = row * old_w * 4;
                    let dst = row * new_w * 4;
                    if let (Some(slice), Some(dst_slice)) = (
                        self.atlas.pixels.get(src..src + new_w * 4),
                        compact.get_mut(dst..dst + new_w * 4),
                    ) {
                        dst_slice.copy_from_slice(slice);
                    }
                }
                self.atlas.pixels = compact;
                self.atlas.size[0] = trimmed_w;
            }
        }
        self.atlas
            .pixels
            .resize((self.atlas.size[0] * self.atlas.size[1] * 4) as usize, 0);
        if self
            .atlas
            .pixels
            .capacity()
            .saturating_sub(self.atlas.pixels.len())
            > 256 * 1024
        {
            self.atlas.pixels.shrink_to_fit();
        }
        #[cfg(not(test))]
        {
            let staged = std::mem::take(&mut self.atlas.pixels);
            self.atlas.gpu_staging = Some(Arc::new(std::sync::Mutex::new(staged)));
        }
        Arc::new(self.atlas)
    }
}
