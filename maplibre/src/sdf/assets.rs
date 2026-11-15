//! Font and sprite pixels packed into a tile's shared symbol atlas.
use std::{collections::HashMap, sync::Arc};

use prost::Message;
use serde::{Deserialize, Serialize};

use crate::sdf::glyphs;

mod cache;
mod load;
mod provider;
mod sprite;
pub mod wire;
#[allow(unused_imports)]
pub(crate) use cache::{fetch, tiles};
pub use cache::{AssetCache, AssetFailure};
#[doc(hidden)]
pub use load::spawn_worker_symbol_prefetch;
#[cfg(target_arch = "wasm32")]
pub(crate) use load::warm_style_symbol_assets;
pub use load::{load_symbol_assets, SymbolAssetConfig, SymbolAssetError};
#[allow(unused_imports)]
pub(crate) use load::{
    load_symbol_assets_awaiting, load_symbol_assets_awaiting_from_tile, spawn_style_symbol_prefetch,
};
pub(crate) use provider::Resolved;
pub use provider::{
    ImageProviderError, ImageProviderStats, ImageProviders, ImageRequest, ImageResolution,
    ProvideFuture, ProvidedImage, ProviderLimits, StyleImageProvider,
};

/// Coordinates and metrics of a glyph or sprite in the atlas.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AtlasEntry {
    /// Pixel rectangle x, y, width, height.
    pub rect: [u32; 4],
    /// Left bearing, top bearing, advance, pixel ratio. An icon has no bearings or advance;
    /// its first two are instead the offset, in image pixels, from its centre to the point
    /// placed where its centre would be.
    pub metrics: [f32; 4],
    /// Zero for text SDF, one for RGBA icons, two for SDF icons.
    pub kind: u32,
    /// How an icon stretches around its text, from the sprite's `stretchX`, `stretchY`,
    /// `content` and `textFit*` fields; `None` for glyphs and plain icons.
    #[serde(default)]
    pub stretch: Option<Box<IconStretch>>,
}

/// The sprite fields that let an icon stretch to fit its text, in image pixels.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct IconStretch {
    /// Column ranges that grow; empty when the icon does not stretch horizontally.
    pub stretch_x: Vec<[f32; 2]>,
    /// Row ranges that grow; empty when the icon does not stretch vertically.
    pub stretch_y: Vec<[f32; 2]>,
    /// The area, `[left, top, right, bottom]`, that the text is fitted to.
    pub content: Option<[f32; 4]>,
    /// How the content area follows the text horizontally.
    pub text_fit_width: Option<TextFit>,
    /// How the content area follows the text vertically.
    pub text_fit_height: Option<TextFit>,
}

/// The `textFitWidth` and `textFitHeight` sprite values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextFit {
    /// The content area may stretch or shrink to the text.
    StretchOrShrink,
    /// The content area may only stretch.
    StretchOnly,
    /// The content area keeps its aspect ratio.
    Proportional,
}

/// Immutable atlas shared by all symbol layers of a tile.
#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct SymbolAtlas {
    /// RGBA pixels.
    pub pixels: Vec<u8>,
    /// Transient RGBA staging buffer on `wasm32` freed immediately after GPU texture upload.
    #[cfg(target_arch = "wasm32")]
    #[serde(skip)]
    pub gpu_staging: Arc<std::sync::Mutex<Vec<u8>>>,
    /// Texture dimensions.
    pub size: [u32; 2],
    /// Font stack and Unicode scalar to atlas entry.
    pub glyphs: HashMap<String, HashMap<u32, AtlasEntry>>,
    /// OpenType glyph IDs (`"<face_id>:<glyph_id>"`) packed for shaped runs (including unencoded
    /// ligature and Indic conjunct glyphs).
    #[serde(default)]
    pub face_glyphs: HashMap<String, AtlasEntry>,
    /// Pre-shaped OpenType glyph runs keyed by font stack and plain transformed label text.
    #[serde(default)]
    pub shaped_runs: HashMap<String, HashMap<String, Vec<crate::font::ShapedGlyph>>>,
    /// Sprite names to atlas entries.
    pub icons: HashMap<String, AtlasEntry>,
}

impl SymbolAtlas {
    /// Creates an atlas whose RGBA bytes are staged for one-shot GPU upload on `wasm32`.
    #[cfg(target_arch = "wasm32")]
    pub fn with_wasm_staging(size: [u32; 2], pixels: Vec<u8>) -> Self {
        Self {
            size,
            gpu_staging: Arc::new(std::sync::Mutex::new(pixels)),
            ..Default::default()
        }
    }

    /// Returns a compact handle sharing only texture dimensions and one-shot GPU staging pixels on
    /// `wasm32`, allowing worker glyph/icon/shaping tables to free immediately after tessellation.
    pub(crate) fn gpu_upload_handle(self: &Arc<Self>) -> Arc<Self> {
        #[cfg(target_arch = "wasm32")]
        {
            Arc::new(Self {
                size: self.size,
                gpu_staging: Arc::clone(&self.gpu_staging),
                pixels: self.pixels.clone(),
                ..Default::default()
            })
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            Arc::clone(self)
        }
    }

    /// Borrows the atlas's RGBA bytes for wire serialization without duplicating them.
    #[cfg(target_arch = "wasm32")]
    pub fn with_pixels<R>(&self, f: impl FnOnce(&[u8]) -> R) -> R {
        if !self.pixels.is_empty() {
            f(&self.pixels)
        } else {
            let guard = crate::platform::lock_unpoisoned(&self.gpu_staging);
            f(&guard)
        }
    }

    /// Takes staged RGBA bytes on `wasm32` (releasing CPU heap immediately once uploaded to GPU)
    /// or borrows `self.pixels`.
    pub(crate) fn take_pixels_for_gpu(&self) -> std::borrow::Cow<'_, [u8]> {
        #[cfg(target_arch = "wasm32")]
        {
            let taken = std::mem::take(&mut *crate::platform::lock_unpoisoned(&self.gpu_staging));
            if !taken.is_empty() {
                return std::borrow::Cow::Owned(taken);
            }
        }
        std::borrow::Cow::Borrowed(&self.pixels)
    }

    /// Estimated CPU storage, counting allocated pixel capacity and glyph metadata.
    pub fn approximate_bytes(&self) -> usize {
        #[cfg(target_arch = "wasm32")]
        let staged_cap = crate::platform::lock_unpoisoned(&self.gpu_staging).capacity();
        #[cfg(not(target_arch = "wasm32"))]
        let staged_cap = 0_usize;
        self.pixels.capacity()
            + staged_cap
            + self
                .glyphs
                .iter()
                .map(|(font, glyphs)| {
                    font.capacity()
                        + glyphs.capacity()
                            * (std::mem::size_of::<u32>() + std::mem::size_of::<AtlasEntry>())
                })
                .sum::<usize>()
            + self
                .face_glyphs
                .keys()
                .map(|key| key.capacity())
                .sum::<usize>()
            + self.face_glyphs.capacity()
                * (std::mem::size_of::<String>() + std::mem::size_of::<AtlasEntry>())
            + self
                .shaped_runs
                .iter()
                .map(|(font, runs)| {
                    font.capacity()
                        + runs
                            .iter()
                            .map(|(text, glyphs)| {
                                text.capacity()
                                    + glyphs.capacity()
                                        * std::mem::size_of::<crate::font::ShapedGlyph>()
                            })
                            .sum::<usize>()
                })
                .sum::<usize>()
            + self.icons.keys().map(|name| name.capacity()).sum::<usize>()
            + self.icons.capacity()
                * (std::mem::size_of::<String>() + std::mem::size_of::<AtlasEntry>())
    }
}

/// Incremental shelf packer with a transparent border around every image.
pub(super) struct AtlasBuilder {
    pub(super) atlas: SymbolAtlas,
    cursor: [u32; 2],
    row_height: u32,
}

#[cfg(target_arch = "wasm32")]
const INITIAL_ATLAS_WIDTH: u32 = 512;
#[cfg(not(target_arch = "wasm32"))]
const INITIAL_ATLAS_WIDTH: u32 = 1024;

impl AtlasBuilder {
    pub(super) fn new() -> Self {
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

    pub(super) fn pack(&mut self, width: u32, height: u32, pixels: &[u8]) -> Option<[u32; 4]> {
        if width > 1022 || height > 4094 || pixels.len() != (width * height * 4) as usize {
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
        self.atlas
            .pixels
            .resize((atlas_height * atlas_width * 4) as usize, 0);
        for row in 0..height {
            let start = (((y + row) * atlas_width + x) * 4) as usize;
            let source = (row * width * 4) as usize;
            self.atlas.pixels[start..start + (width * 4) as usize]
                .copy_from_slice(&pixels[source..source + (width * 4) as usize]);
        }
        self.cursor[0] += width + 2;
        self.row_height = self.row_height.max(height);
        self.atlas.size[1] = atlas_height;
        Some([x, y, width, height])
    }

    pub(super) fn glyph_range(
        &mut self,
        font: &str,
        bytes: &[u8],
    ) -> Result<(), prost::DecodeError> {
        self.glyph_subset(font, &glyphs::Glyphs::decode(bytes)?, None);
        Ok(())
    }

    pub(super) fn glyph_subset(
        &mut self,
        font: &str,
        data: &glyphs::Glyphs,
        wanted: Option<&std::collections::BTreeSet<u32>>,
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
                    let pixels: Vec<u8> = bitmap
                        .iter()
                        .flat_map(|&value| [value, value, value, 255])
                        .collect();
                    let Some(rect) = self.pack(glyph.width + 6, glyph.height + 6, &pixels) else {
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

    pub(super) fn pack_rasterized_codepoint(
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

    pub(super) fn pack_rasterized_face_glyph(
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
            let pixels: Vec<u8> = glyph
                .bitmap
                .iter()
                .flat_map(|&value| [value, value, value, 255])
                .collect();
            self.pack(glyph.width + 6, glyph.height + 6, &pixels)?
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

    pub(super) fn finish(mut self) -> Arc<SymbolAtlas> {
        self.atlas.size[1] = self.atlas.size[1].max(1);
        #[cfg(target_arch = "wasm32")]
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
                    if let Some(slice) = self.atlas.pixels.get(src..src + new_w * 4) {
                        compact[dst..dst + new_w * 4].copy_from_slice(slice);
                    }
                }
                self.atlas.pixels = compact;
                self.atlas.size[0] = trimmed_w;
            }
        }
        self.atlas
            .pixels
            .resize((self.atlas.size[0] * self.atlas.size[1] * 4) as usize, 0);
        self.atlas.pixels.shrink_to_fit();
        #[cfg(target_arch = "wasm32")]
        {
            let staged = std::mem::take(&mut self.atlas.pixels);
            self.atlas.gpu_staging = Arc::new(std::sync::Mutex::new(staged));
        }
        Arc::new(self.atlas)
    }
}

/// Bundled glyphs keep offline styles usable when no glyph URL is supplied.
pub fn fallback_atlas() -> Arc<SymbolAtlas> {
    let mut builder = AtlasBuilder::new();
    if let Err(error) = builder.glyph_range(
        "Open Sans Regular",
        include_bytes!("../../../data/0-255.pbf"),
    ) {
        tracing::error!(%error, "bundled symbol glyphs are invalid");
    }
    builder.finish()
}

#[cfg(test)]
mod tests;
