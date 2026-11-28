//! Internal pixel packing and entry-insertion helpers for [`SymbolAtlas`].

#[cfg(not(test))]
use std::sync::Arc;

use super::{AtlasEntry, SymbolAtlas};

impl SymbolAtlas {
    pub(in crate::layers::sdf::assets) fn new_empty(width: u32) -> Self {
        Self {
            size: [width, 1],
            ..Default::default()
        }
    }

    pub(in crate::layers::sdf::assets) fn widen_to_1024(&mut self) {
        let [old_w, old_h] = self.size;
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
                self.pixels.get(src..src + old_row_bytes),
                widened.get_mut(dst..dst + old_row_bytes),
            ) {
                dst_slice.copy_from_slice(src_slice);
            }
        }
        self.pixels = widened;
        self.size[0] = new_w;
    }

    pub(in crate::layers::sdf::assets) fn grow_height_for_packing(
        &mut self,
        atlas_height: u32,
        max_overalloc: usize,
    ) {
        let atlas_width = self.size[0];
        let needed = (atlas_height * atlas_width * 4) as usize;
        if needed > self.pixels.capacity() {
            let target = needed
                .saturating_mul(2)
                .min(needed.saturating_add(max_overalloc));
            self.pixels
                .reserve(target.saturating_sub(self.pixels.len()));
        }
        self.pixels.resize(needed, 0);
        self.size[1] = atlas_height;
    }

    pub(in crate::layers::sdf::assets) fn write_rgba_rect(
        &mut self,
        [x, y, width, height]: [u32; 4],
        atlas_width: u32,
        pixels: &[u8],
    ) -> bool {
        let row_bytes = (width * 4) as usize;
        for row in 0..height {
            let start = (((y + row) * atlas_width + x) * 4) as usize;
            let source = (row * width * 4) as usize;
            let (Some(dst), Some(src)) = (
                self.pixels.get_mut(start..start + row_bytes),
                pixels.get(source..source + row_bytes),
            ) else {
                return false;
            };
            dst.copy_from_slice(src);
        }
        true
    }

    pub(in crate::layers::sdf::assets) fn write_alpha_sdf_rect(
        &mut self,
        [x, y, width, height]: [u32; 4],
        atlas_width: u32,
        alpha: &[u8],
    ) -> bool {
        let row_bytes = (width * 4) as usize;
        for row in 0..height {
            let start = (((y + row) * atlas_width + x) * 4) as usize;
            let src_start = (row * width) as usize;
            let (Some(src), Some(dst)) = (
                alpha.get(src_start..src_start + width as usize),
                self.pixels.get_mut(start..start + row_bytes),
            ) else {
                return false;
            };
            for (&a, px) in src.iter().zip(dst.as_chunks_mut::<4>().0) {
                *px = [a, a, a, 255];
            }
        }
        true
    }

    pub(in crate::layers::sdf::assets) fn write_cropped_rgba_rect(
        &mut self,
        [x, y, width, height]: [u32; 4],
        atlas_width: u32,
        image: &image::RgbaImage,
        [sx, sy]: [u32; 2],
        premul: bool,
    ) -> bool {
        let img_stride = (image.width() * 4) as usize;
        let raw = image.as_raw();
        let row_bytes = (width * 4) as usize;
        for row in 0..height {
            let dst_start = (((y + row) * atlas_width + x) * 4) as usize;
            let src_start = (sy + row) as usize * img_stride + (sx * 4) as usize;
            let (Some(src), Some(dst)) = (
                raw.get(src_start..src_start + row_bytes),
                self.pixels.get_mut(dst_start..dst_start + row_bytes),
            ) else {
                return false;
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
        true
    }

    pub(in crate::layers::sdf::assets) fn insert_glyph(
        &mut self,
        font: &str,
        id: u32,
        entry: AtlasEntry,
    ) {
        self.glyphs
            .entry(font.to_owned())
            .or_default()
            .insert(id, entry);
    }

    pub(in crate::layers::sdf::assets) fn contains_glyph(&self, font: &str, id: u32) -> bool {
        self.glyphs
            .get(font)
            .is_some_and(|map| map.contains_key(&id))
    }

    pub(in crate::layers::sdf::assets) fn contains_face_glyph(&self, key: &str) -> bool {
        self.face_glyphs.contains_key(key)
    }

    pub(in crate::layers::sdf::assets) fn insert_face_glyph(
        &mut self,
        key: String,
        entry: AtlasEntry,
    ) {
        self.face_glyphs.insert(key, entry);
    }

    pub(in crate::layers::sdf::assets) fn insert_shaped_run(
        &mut self,
        vertical: bool,
        font: &str,
        label: &str,
        shaped: Vec<crate::font::ShapedGlyph>,
    ) {
        let target = if vertical {
            &mut self.vertical_shaped_runs
        } else {
            &mut self.shaped_runs
        };
        target
            .entry(font.to_owned())
            .or_default()
            .insert(label.to_owned(), shaped);
    }

    pub(in crate::layers::sdf::assets) fn contains_icon(&self, name: &str) -> bool {
        self.icons.contains_key(name)
    }

    pub(in crate::layers::sdf::assets) fn insert_icon(&mut self, name: String, entry: AtlasEntry) {
        self.icons.insert(name, entry);
    }

    pub(in crate::layers::sdf::assets) fn finalize_pixels(
        &mut self,
        cursor_x: u32,
        single_row: bool,
    ) {
        self.size[1] = self.size[1].max(1);
        if single_row {
            let trimmed_w = cursor_x.max(4).next_multiple_of(4).min(self.size[0]);
            if trimmed_w < self.size[0] {
                let old_w = self.size[0] as usize;
                let h = self.size[1] as usize;
                let new_w = trimmed_w as usize;
                let mut compact = vec![0_u8; new_w * h * 4];
                for row in 0..h {
                    let src = row * old_w * 4;
                    let dst = row * new_w * 4;
                    if let (Some(slice), Some(dst_slice)) = (
                        self.pixels.get(src..src + new_w * 4),
                        compact.get_mut(dst..dst + new_w * 4),
                    ) {
                        dst_slice.copy_from_slice(slice);
                    }
                }
                self.pixels = compact;
                self.size[0] = trimmed_w;
            }
        }
        self.pixels
            .resize((self.size[0] * self.size[1] * 4) as usize, 0);
        if self.pixels.capacity().saturating_sub(self.pixels.len()) > 256 * 1024 {
            self.pixels.shrink_to_fit();
        }
        #[cfg(not(test))]
        {
            let staged = std::mem::take(&mut self.pixels);
            self.gpu_staging = Some(Arc::new(std::sync::Mutex::new(staged)));
        }
    }
}
