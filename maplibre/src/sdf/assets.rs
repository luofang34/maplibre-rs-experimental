//! Font and sprite pixels packed into a tile's shared symbol atlas.
use crate::sdf::text::sdf_glyphs;
use prost::Message;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};

mod load;
pub mod wire;
pub use load::load_symbol_assets;

/// Coordinates and metrics of a glyph or sprite in the atlas.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AtlasEntry {
    /// Pixel rectangle x, y, width, height.
    pub rect: [u32; 4],
    /// Left bearing, top bearing, advance, pixel ratio.
    pub metrics: [f32; 4],
    /// Zero for text SDF, one for RGBA icons, two for SDF icons.
    pub kind: u32,
}

/// Immutable atlas shared by all symbol layers of a tile.
#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct SymbolAtlas {
    /// RGBA pixels.
    pub pixels: Vec<u8>,
    /// Texture dimensions.
    pub size: [u32; 2],
    /// Font stack and Unicode scalar to atlas entry.
    pub glyphs: HashMap<String, HashMap<u32, AtlasEntry>>,
    /// Sprite names to atlas entries.
    pub icons: HashMap<String, AtlasEntry>,
}

impl SymbolAtlas {
    /// Estimated CPU storage, counting allocated pixel capacity and glyph metadata.
    pub fn approximate_bytes(&self) -> usize {
        self.pixels.capacity()
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
                .icons
                .iter()
                .map(|(name, _)| name.capacity())
                .sum::<usize>()
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

impl AtlasBuilder {
    pub(super) fn new() -> Self {
        Self {
            atlas: SymbolAtlas {
                size: [1024, 1],
                ..Default::default()
            },
            cursor: [1, 1],
            row_height: 0,
        }
    }

    pub(super) fn pack(&mut self, width: u32, height: u32, pixels: &[u8]) -> Option<[u32; 4]> {
        let [atlas_width, _] = self.atlas.size;
        if width > atlas_width - 2 || height > 4094 || pixels.len() != (width * height * 4) as usize
        {
            return None;
        }
        if self.cursor[0] + width + 1 > atlas_width {
            self.cursor = [1, self.cursor[1] + self.row_height + 2];
            self.row_height = 0;
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
        self.glyph_subset(font, bytes, None)
    }

    pub(super) fn glyph_subset(
        &mut self,
        font: &str,
        bytes: &[u8],
        wanted: Option<&std::collections::BTreeSet<u32>>,
    ) -> Result<(), prost::DecodeError> {
        let data = sdf_glyphs::Glyphs::decode(bytes)?;
        for glyph in data.stacks.into_iter().flat_map(|stack| stack.glyphs) {
            if wanted.is_some_and(|wanted| !wanted.contains(&glyph.id)) {
                continue;
            }
            if glyph.width > 1016 || glyph.height > 4088 {
                continue;
            }
            let rect = if let Some(bitmap) = glyph.bitmap.filter(|bitmap| !bitmap.is_empty()) {
                let pixels: Vec<u8> = bitmap
                    .into_iter()
                    .flat_map(|value| [value, value, value, 255])
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
                    },
                );
        }
        Ok(())
    }

    pub(super) fn finish(mut self) -> Arc<SymbolAtlas> {
        self.atlas.size[1] = self.atlas.size[1].max(1);
        self.atlas
            .pixels
            .resize((self.atlas.size[0] * self.atlas.size[1] * 4) as usize, 0);
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
