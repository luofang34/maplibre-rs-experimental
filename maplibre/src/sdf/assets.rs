//! Font and sprite pixels packed into a tile's shared symbol atlas.
use std::sync::Arc;

use rustc_hash::FxHashMap as HashMap;
use serde::{Deserialize, Serialize};

mod builder;
mod cache;
mod load;
mod provider;
mod sprite;
pub mod wire;
pub(super) use builder::AtlasBuilder;
#[cfg(feature = "raster")]
pub(crate) use cache::fetch;
pub(crate) use cache::tiles;
pub use cache::{AssetCache, AssetFailure};
#[cfg(not(all(target_arch = "wasm32", target_feature = "atomics")))]
pub(crate) use load::spawn_style_symbol_prefetch;
#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
pub(crate) use load::PrefetchPlan;
pub(crate) use load::{
    fetch_symbol_asset_inputs, load_symbol_assets_awaiting_from_tile, pack_symbol_assets,
    symbol_requests_from_tile,
};
pub use load::{
    load_symbol_assets, spawn_worker_symbol_prefetch, SymbolAssetConfig, SymbolAssetError,
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
    /// Transient RGBA staging buffer freed immediately after GPU texture upload.
    #[serde(skip)]
    pub gpu_staging: Option<Arc<std::sync::Mutex<Vec<u8>>>>,
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
    /// Pre-shaped vertical OpenType glyph runs (`Direction::TopToBottom` + `vert`/`vrt2`) keyed
    /// by font stack and plain transformed label text.
    #[serde(default)]
    pub vertical_shaped_runs: HashMap<String, HashMap<String, Vec<crate::font::ShapedGlyph>>>,
    /// Sprite names to atlas entries.
    pub icons: HashMap<String, AtlasEntry>,
}

impl SymbolAtlas {
    /// Creates an atlas whose RGBA bytes are staged for one-shot GPU upload on `wasm32`.
    #[cfg(target_arch = "wasm32")]
    pub fn with_wasm_staging(size: [u32; 2], pixels: Vec<u8>) -> Self {
        Self {
            size,
            gpu_staging: Some(Arc::new(std::sync::Mutex::new(pixels))),
            ..Default::default()
        }
    }

    /// Returns a compact handle sharing only texture dimensions and one-shot GPU staging pixels,
    /// allowing worker glyph/icon/shaping tables to free immediately after tessellation.
    pub(crate) fn gpu_upload_handle(self: &Arc<Self>) -> Arc<Self> {
        #[cfg(not(test))]
        {
            Arc::new(Self {
                size: self.size,
                gpu_staging: self.gpu_staging.clone(),
                pixels: if self.gpu_staging.is_some() {
                    Vec::new()
                } else {
                    self.pixels.clone()
                },
                ..Default::default()
            })
        }
        #[cfg(test)]
        {
            Arc::clone(self)
        }
    }

    /// Borrows the atlas's RGBA bytes for wire serialization without duplicating them.
    #[cfg(target_arch = "wasm32")]
    pub fn with_pixels<R>(&self, f: impl FnOnce(&[u8]) -> R) -> R {
        if !self.pixels.is_empty() {
            f(&self.pixels)
        } else if let Some(staging) = &self.gpu_staging {
            let guard = crate::platform::lock_unpoisoned(staging);
            f(&guard)
        } else {
            f(&[])
        }
    }

    /// Takes staged RGBA bytes (releasing CPU heap immediately once uploaded to GPU)
    /// or borrows `self.pixels`.
    pub(crate) fn take_pixels_for_gpu(&self) -> std::borrow::Cow<'_, [u8]> {
        if let Some(staging) = &self.gpu_staging {
            let taken = std::mem::take(&mut *crate::platform::lock_unpoisoned(staging));
            if !taken.is_empty() {
                return std::borrow::Cow::Owned(taken);
            }
        }
        std::borrow::Cow::Borrowed(&self.pixels)
    }

    /// Stable identity key shared across compact `gpu_upload_handle` clones.
    pub(crate) fn gpu_identity(self: &Arc<Self>) -> usize {
        self.gpu_staging
            .as_ref()
            .map_or(Arc::as_ptr(self) as usize, |s| Arc::as_ptr(s) as usize)
    }

    /// Returns `true` when `self` and `other` reference the same underlying atlas pixels/staging.
    pub(crate) fn same_gpu_atlas(self: &Arc<Self>, other: &Arc<Self>) -> bool {
        match (&self.gpu_staging, &other.gpu_staging) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => Arc::ptr_eq(self, other),
            _ => false,
        }
    }

    /// Estimated CPU storage, counting allocated pixel capacity and glyph metadata.
    pub fn approximate_bytes(&self) -> usize {
        let staged_cap = self
            .gpu_staging
            .as_ref()
            .map_or(0, |s| crate::platform::lock_unpoisoned(s).capacity());
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
