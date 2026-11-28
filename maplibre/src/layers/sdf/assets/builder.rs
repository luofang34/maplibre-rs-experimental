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
pub(crate) struct AtlasBuilder {
    atlas: SymbolAtlas,
    cursor: [u32; 2],
    row_height: u32,
}

impl AtlasBuilder {
    pub(crate) fn new() -> Self {
        Self {
            atlas: SymbolAtlas::new_empty(INITIAL_ATLAS_WIDTH),
            cursor: [1, 1],
            row_height: 0,
        }
    }

    pub(in crate::layers::sdf::assets) fn contains_icon(&self, name: &str) -> bool {
        self.atlas.contains_icon(name)
    }

    pub(crate) fn insert_icon(&mut self, name: String, entry: AtlasEntry) {
        self.atlas.insert_icon(name, entry)
    }

    pub(in crate::layers::sdf::assets) fn contains_glyph(&self, font: &str, id: u32) -> bool {
        self.atlas.contains_glyph(font, id)
    }

    pub(in crate::layers::sdf::assets) fn contains_face_glyph(&self, key: &str) -> bool {
        self.atlas.contains_face_glyph(key)
    }

    pub(in crate::layers::sdf::assets) fn insert_shaped_run(
        &mut self,
        vertical: bool,
        font: &str,
        label: &str,
        shaped: Vec<crate::font::ShapedGlyph>,
    ) {
        self.atlas.insert_shaped_run(vertical, font, label, shaped)
    }

    fn allocate_rect(&mut self, width: u32, height: u32) -> Option<([u32; 2], u32)> {
        if width > 1022 || height > 4094 {
            return None;
        }
        if width > self.atlas.dimensions().0 - 2 {
            self.atlas.widen_to_1024();
        }
        let (mut atlas_width, cur_h) = self.atlas.dimensions();
        if self.cursor[0] + width + 1 > atlas_width {
            let next_y = self.cursor[1] + self.row_height + 2;
            if next_y + height + 1 > 4096 && atlas_width < 1024 {
                self.atlas.widen_to_1024();
                atlas_width = self.atlas.dimensions().0;
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
        let atlas_height = cur_h.max(y + height + 1);
        self.atlas
            .grow_height_for_packing(atlas_height, MAX_PIXELS_OVERALLOC);
        self.cursor[0] += width + 2;
        self.row_height = self.row_height.max(height);
        Some(([x, y], atlas_width))
    }

    pub(crate) fn pack(&mut self, width: u32, height: u32, pixels: &[u8]) -> Option<[u32; 4]> {
        if width > 1022 || height > 4094 || pixels.len() != (width as usize) * (height as usize) * 4
        {
            return None;
        }
        let ([x, y], atlas_width) = self.allocate_rect(width, height)?;
        let rect = [x, y, width, height];
        self.atlas
            .write_rgba_rect(rect, atlas_width, pixels)
            .then_some(rect)
    }

    /// Writes single-channel SDF `alpha` bytes directly into RGBA atlas memory without a temporary `Vec<u8>`.
    pub(in crate::layers::sdf::assets) fn pack_alpha_sdf(
        &mut self,
        width: u32,
        height: u32,
        alpha: &[u8],
    ) -> Option<[u32; 4]> {
        if width > 1022 || height > 4094 || alpha.len() != (width as usize) * (height as usize) {
            return None;
        }
        let ([x, y], atlas_width) = self.allocate_rect(width, height)?;
        let rect = [x, y, width, height];
        self.atlas
            .write_alpha_sdf_rect(rect, atlas_width, alpha)
            .then_some(rect)
    }

    /// Copies a sub-rectangle directly from `image` into the atlas, optionally premultiplying RGB by alpha.
    pub(in crate::layers::sdf::assets) fn pack_cropped_rgba(
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
        let rect = [x, y, width, height];
        self.atlas
            .write_cropped_rgba_rect(rect, atlas_width, image, [sx, sy], premul)
            .then_some(rect)
    }

    pub(crate) fn glyph_range(
        &mut self,
        font: &str,
        bytes: &[u8],
    ) -> Result<(), prost::DecodeError> {
        self.glyph_subset(font, &glyphs::Glyphs::decode(bytes)?, None);
        Ok(())
    }

    pub(in crate::layers::sdf::assets) fn glyph_subset(
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
            self.atlas.insert_glyph(
                font,
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

    pub(in crate::layers::sdf::assets) fn pack_rasterized_codepoint(
        &mut self,
        font: &str,
        id: u32,
        glyph: &crate::font::RasterizedGlyph,
    ) {
        if let Some(entry) = self.entry_for_rasterized(glyph) {
            self.atlas.insert_glyph(font, id, entry);
        }
    }

    pub(in crate::layers::sdf::assets) fn pack_rasterized_face_glyph(
        &mut self,
        key: String,
        glyph: &crate::font::RasterizedGlyph,
    ) {
        if self.atlas.contains_face_glyph(&key) {
            return;
        }
        if let Some(entry) = self.entry_for_rasterized(glyph) {
            self.atlas.insert_face_glyph(key, entry);
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

    pub(crate) fn finish(mut self) -> Arc<SymbolAtlas> {
        self.atlas
            .finalize_pixels(self.cursor[0], self.cursor[1] == 1);
        Arc::new(self.atlas)
    }
}
